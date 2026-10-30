//! Modbus RTU: the box as master on a serial line (RS-485 two-wire, or
//! RS-232), with the same application layer as Modbus TCP
//! ([`crate::modbus::app`], points and PDUs from [`crate::modbus`]).
//!
//! References (see docs/SOURCES.md):
//! - MODBUS over Serial Line Specification and Implementation Guide V1.02
//!   (Modbus.org, 2006-12-20), cited as "Serial §x p.y".
//! - MODBUS Application Protocol Specification V1.1b3, cited as "App §x p.y".
//!
//! What this module does, and where the spec says so:
//!
//! - **Frame**: unit address, PDU, CRC-16 low byte first; at most 256 bytes
//!   (Serial §2.5.1 p.13, §2.5.1.2 p.14). The CRC is the reflected
//!   polynomial 0xA001 from 0xFFFF (Serial §6.2.2 pp.39-41).
//! - **Addresses**: 0 is broadcast, 1..=247 a unit, 248..=255 reserved
//!   (Serial §2.2 p.8). A broadcast is a write only, and nothing replies
//!   (Serial §2.1 p.7): it returns as soon as it is sent, and the next
//!   request waits the *turnaround delay* (Serial §2.4.1 pp.9-10).
//! - **Timing**: a frame starts only after the line has been idle for
//!   t3.5 (3.5 character times; 11 bits per character, Serial §2.5.1 p.12;
//!   fixed 1.75 ms above 19200 baud, Serial §2.5.1.1 p.13). The response
//!   time-out starts once the request is sent, and keeps running past a
//!   reply from an unexpected unit (Serial §2.4.1 p.9).
//! - **Receiving**: the reply's length follows from its function code and
//!   byte count; the receiver hunts for the unit and function it expects
//!   and takes the first such frame whose CRC checks. Bytes before it
//!   (noise, a reply from another unit, a corrupted frame) are dropped.
//!   With `timing=strict` a silence over t1.5 inside a frame discards it,
//!   as Serial §2.5.1.1 p.13 says; it is off by default because a host
//!   behind a USB serial bridge sees a reply in bursts several milliseconds
//!   apart (the bridge's latency timer), which strict timing would discard.
//! - **After a link error** (time-out, CRC, a truncated frame) the line is
//!   drained until it has been quiet for one response time-out before the
//!   next request, so a late reply can't be taken for the answer to a newer
//!   request (the RTU frame has no transaction id).
//!
//! Device params (`device ID modbus-rtu /dev/ttyUSB0 KEY=VALUE ...`):
//! `unit` (1; 0 = broadcast, writes only), `baud` (19200, the required
//! default of Serial §3.2 p.20), `parity` (`even`, the required default of
//! Serial §2.5.1 p.12; `odd`; `none`), `stop` (1, or 2 with no parity, as
//! Serial §2.5.1 p.12 requires), `timeout_ms` (1000), `turnaround_ms` (100,
//! Serial §2.4.1 p.10 gives 100-200 ms), `timing` (`relaxed` or `strict`).

use std::collections::BTreeMap;
use std::io;
use std::thread;
use std::time::{Duration, Instant};

use crate::driver::{Driver, Error};
use crate::modbus::{
    MAX_PDU, ModbusDriver, Transport, WRITE_MULTIPLE_REGISTERS, WRITE_SINGLE_COIL, WRITE_SINGLE_REGISTER,
};

pub mod pipe;
#[cfg(all(unix, feature = "serial"))]
pub mod serial;
pub mod sim;

pub const DRIVER: &str = "modbus-rtu";
/// Serial §2.5.1 p.13: address (1) + PDU (at most 253) + CRC (2).
pub const MAX_ADU: usize = 256;
/// Serial §2.2 p.8.
pub const BROADCAST: u8 = 0;
/// Serial §2.2 p.8: the highest individual unit address.
pub const MAX_UNIT: u8 = 247;
/// Serial §3.2 p.20: 19200 is the required default.
pub const DEFAULT_BAUD: u32 = 19_200;
/// Write Multiple Coils: not sent by this master, but a write a broadcast
/// may carry (Serial §2.1 p.7 allows only writes).
const WRITE_MULTIPLE_COILS: u8 = 0x0F;

/// CRC-16/MODBUS: preset 0xFFFF, each byte XOR-ed into the low byte, eight
/// shifts towards the LSB, XOR with 0xA001 when the shifted-out bit is 1
/// (Serial §6.2.2 pp.39-40). Sent low byte first (Serial §2.5.1.2 p.14).
#[must_use]
pub fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0xFFFF_u16;
    for &b in bytes {
        crc ^= u16::from(b);
        for _ in 0..8 {
            let carry = crc & 1;
            crc >>= 1;
            if carry == 1 {
                crc ^= 0xA001;
            }
        }
    }
    crc
}

/// One RTU frame: unit, PDU, CRC.
///
/// # Errors
/// A PDU that is empty or over 253 bytes (App §4.1 p.3).
pub fn encode(unit: u8, pdu: &[u8]) -> Result<Vec<u8>, Error> {
    if pdu.is_empty() || pdu.len() > MAX_PDU {
        return Err(Error::Protocol(format!("a PDU of {} bytes", pdu.len())));
    }
    let mut f = Vec::with_capacity(pdu.len() + 3);
    f.push(unit);
    f.extend_from_slice(pdu);
    f.extend(crc16(&f).to_le_bytes());
    Ok(f)
}

/// A whole frame back into `(unit, PDU)`.
///
/// # Errors
/// Shorter than address + function + CRC, longer than 256 bytes, or a CRC
/// that does not check.
pub fn decode(frame: &[u8]) -> Result<(u8, &[u8]), Error> {
    if frame.len() < 4 || frame.len() > MAX_ADU {
        return Err(Error::Protocol(format!("a frame of {} bytes", frame.len())));
    }
    let (body, crc) = frame.split_at(frame.len() - 2);
    if crc16(body).to_le_bytes() != crc {
        return Err(Error::Protocol("CRC error".into()));
    }
    Ok((body[0], &body[1..]))
}

/// The length of a reply frame from its first bytes (address, function,
/// and for reads the byte count): `None` until enough are in, or for a
/// function this master never sends. Exceptions are address, function |
/// 0x80, code, CRC (App §7 p.47); reads are 3 + byte count + 2 (App
/// §6.1-6.4); single writes and 0x0F/0x10 replies are 8 (App §6.5, §6.6,
/// §6.11, §6.12).
#[must_use]
pub fn reply_len(head: &[u8]) -> Option<usize> {
    let fc = *head.get(1)?;
    if fc & 0x80 != 0 {
        return Some(5);
    }
    match fc {
        0x01..=0x04 => head.get(2).map(|&n| 5 + usize::from(n)),
        WRITE_SINGLE_COIL | WRITE_SINGLE_REGISTER | WRITE_MULTIPLE_COILS | WRITE_MULTIPLE_REGISTERS => {
            Some(8)
        }
        _ => None,
    }
}

/// t1.5 and t3.5 at a baud rate (Serial §2.5.1.1 p.13): 1.5 and 3.5
/// eleven-bit characters up to 19200 baud, 750 µs and 1.75 ms above.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    pub t15: Duration,
    pub t35: Duration,
}

impl Timing {
    /// # Panics
    /// Never for `baud >= 1`; [`Params::parse`] refuses 0.
    #[must_use]
    pub fn for_baud(baud: u32) -> Self {
        if baud > DEFAULT_BAUD {
            return Self {
                t15: Duration::from_micros(750),
                t35: Duration::from_micros(1750),
            };
        }
        // 11 bits per character (Serial §2.5.1 p.12), in nanoseconds.
        let char_ns = 11_000_000_000 / u64::from(baud.max(1));
        Self {
            t15: Duration::from_nanos(char_ns * 3 / 2),
            t35: Duration::from_nanos(char_ns * 7 / 2),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parity {
    Even,
    Odd,
    None,
}

/// The serial character format: always 8 data bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub baud: u32,
    pub parity: Parity,
    pub stop_bits: u8,
}

/// A `modbus-rtu` device's params; see the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    pub unit: u8,
    pub settings: Settings,
    pub timeout: Duration,
    pub turnaround: Duration,
    pub strict: bool,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            unit: 1,
            settings: Settings {
                baud: DEFAULT_BAUD,
                parity: Parity::Even,
                stop_bits: 1,
            },
            timeout: Duration::from_secs(1),
            turnaround: Duration::from_millis(100),
            strict: false,
        }
    }
}

impl Params {
    /// From device params; keys it does not know (`poll_ms`) are the link's.
    ///
    /// # Errors
    /// [`Error::Address`] naming the param that does not parse or is out
    /// of range.
    pub fn parse(params: &BTreeMap<String, String>) -> Result<Self, Error> {
        let mut p = Self::default();
        let bad = |k: &str, v: &str| Error::Address(format!("{k}={v}"));
        let num = |k: &str, v: &str| -> Result<u64, Error> {
            if v.is_empty() || v.len() > 10 || !v.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad(k, v));
            }
            v.parse().map_err(|_| bad(k, v))
        };
        if let Some(v) = params.get("unit") {
            p.unit = u8::try_from(num("unit", v)?)
                .ok()
                .filter(|u| *u <= MAX_UNIT)
                .ok_or_else(|| bad("unit", v))?;
        }
        if let Some(v) = params.get("baud") {
            p.settings.baud = u32::try_from(num("baud", v)?)
                .ok()
                .filter(|b| (50..=4_000_000).contains(b))
                .ok_or_else(|| bad("baud", v))?;
        }
        if let Some(v) = params.get("parity") {
            p.settings.parity = match v.as_str() {
                "even" => Parity::Even,
                "odd" => Parity::Odd,
                "none" => Parity::None,
                _ => return Err(bad("parity", v)),
            };
        }
        p.settings.stop_bits = match params.get("stop").map(String::as_str) {
            None if p.settings.parity == Parity::None => 2,
            None | Some("1") => 1,
            Some("2") => 2,
            Some(v) => return Err(bad("stop", v)),
        };
        if let Some(v) = params.get("timeout_ms") {
            let ms = num("timeout_ms", v)?;
            if !(1..=60_000).contains(&ms) {
                return Err(bad("timeout_ms", v));
            }
            p.timeout = Duration::from_millis(ms);
        }
        if let Some(v) = params.get("turnaround_ms") {
            let ms = num("turnaround_ms", v)?;
            if ms > 10_000 {
                return Err(bad("turnaround_ms", v));
            }
            p.turnaround = Duration::from_millis(ms);
        }
        if let Some(v) = params.get("timing") {
            p.strict = match v.as_str() {
                "relaxed" => false,
                "strict" => true,
                _ => return Err(bad("timing", v)),
            };
        }
        Ok(p)
    }
}

/// Collects a reply to one request from bytes as they arrive. Pure: time
/// comes in with the bytes, so every framing rule is testable without a
/// clock.
#[derive(Debug)]
pub struct Receiver {
    unit: u8,
    fc: u8,
    max_gap: Option<Duration>,
    buf: Vec<u8>,
    last: Option<Instant>,
    bad_frames: usize,
}

impl Receiver {
    /// Expects a reply from `unit` to function `fc`. `max_gap` is the
    /// longest silence allowed inside a frame (t1.5 when strict).
    #[must_use]
    pub fn new(unit: u8, fc: u8, max_gap: Option<Duration>) -> Self {
        Self {
            unit,
            fc,
            max_gap,
            buf: Vec::new(),
            last: None,
            bad_frames: 0,
        }
    }

    fn starts_reply(&self, b: &[u8]) -> bool {
        match b {
            [] => false,
            [u] => *u == self.unit,
            [u, f, ..] => *u == self.unit && (*f == self.fc || *f == self.fc | 0x80),
        }
    }

    /// Feeds bytes that arrived together at `at`; returns the reply PDU
    /// once a whole frame from the expected unit checks.
    pub fn push(&mut self, chunk: &[u8], at: Instant) -> Option<Vec<u8>> {
        if let (Some(gap), Some(last)) = (self.max_gap, self.last)
            && at.saturating_duration_since(last) > gap
            && !self.buf.is_empty()
        {
            // Serial §2.5.1.1 p.13: a silence over t1.5 makes the frame
            // incomplete; it is discarded.
            self.bad_frames += 1;
            self.buf.clear();
        }
        self.last = Some(at);
        for &b in chunk {
            self.buf.push(b);
            if let Some(pdu) = self.scan() {
                return Some(pdu);
            }
        }
        None
    }

    /// Called after each byte: any frame ends at the newest byte, so check
    /// every place one could have started. A false start (noise that looks
    /// like the unit and function, with a byte count running past the real
    /// reply) never hides a real reply that starts after it.
    fn scan(&mut self) -> Option<Vec<u8>> {
        let end = self.buf.len();
        for i in 0..end {
            let tail = &self.buf[i..];
            if self.starts_reply(tail) && reply_len(tail) == Some(tail.len()) {
                if let Ok((_, pdu)) = decode(tail) {
                    return Some(pdu.to_vec());
                }
                // A CRC error, or bytes that only looked like a start.
                self.bad_frames += 1;
            }
        }
        // Forget what can no longer start a frame: the buffer stays under
        // one frame's length plus the bytes since.
        let keep = (0..end)
            .find(|&i| {
                let tail = &self.buf[i..];
                self.starts_reply(tail) && reply_len(tail).is_none_or(|n| n > tail.len())
            })
            .unwrap_or(end);
        self.buf.drain(..keep);
        None
    }

    /// The error when the response time-out runs out first: a partial
    /// frame or a frame that failed its CRC is a protocol error, silence a
    /// time-out. Both are link errors.
    #[must_use]
    pub fn give_up(&self, timeout: Duration) -> Error {
        if !self.buf.is_empty() {
            Error::Protocol(format!(
                "incomplete frame: {} bytes from unit {} when the {} ms time-out ran out",
                self.buf.len(),
                self.unit,
                timeout.as_millis()
            ))
        } else if self.bad_frames > 0 {
            Error::Protocol(format!(
                "no valid reply from unit {}: {} frame(s) failed the CRC or framing",
                self.unit, self.bad_frames
            ))
        } else {
            Error::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("no reply from unit {} in {} ms", self.unit, timeout.as_millis()),
            ))
        }
    }
}

/// A serial line as the master sees it: bytes out, bytes in with a wait.
/// The real port is [`serial::SerialLine`]; tests use [`pipe::End`].
pub trait Line: Send {
    /// Sends a whole frame, returning once it is on its way out.
    ///
    /// # Errors
    /// The port failed.
    fn send(&mut self, frame: &[u8]) -> io::Result<()>;

    /// Waits up to `wait` for bytes; `Ok(0)` when none came.
    ///
    /// # Errors
    /// The port failed or went away.
    fn recv(&mut self, buf: &mut [u8], wait: Duration) -> io::Result<usize>;
}

/// An RTU master for one unit on a line.
pub struct RtuClient<L> {
    line: L,
    params: Params,
    timing: Timing,
    /// No request goes out before this: t3.5 after the last reply, or the
    /// turnaround delay after a broadcast.
    ready_at: Instant,
    /// A link error happened; drain before the next request.
    dirty: bool,
}

impl<L: Line> RtuClient<L> {
    #[must_use]
    pub fn new(line: L, params: Params) -> Self {
        Self {
            line,
            timing: Timing::for_baud(params.settings.baud),
            params,
            ready_at: Instant::now(),
            dirty: false,
        }
    }

    pub fn line_mut(&mut self) -> &mut L {
        &mut self.line
    }

    #[must_use]
    pub const fn params(&self) -> &Params {
        &self.params
    }

    /// Reads and drops whatever arrives until the line has been quiet for
    /// `quiet`, giving up (a link error) if it never is.
    fn drain(&mut self, quiet: Duration) -> Result<(), Error> {
        let limit = Instant::now() + quiet + self.params.timeout;
        let mut buf = [0u8; MAX_ADU];
        while self.line.recv(&mut buf, quiet)? > 0 {
            if Instant::now() > limit {
                return Err(Error::Protocol("the line never goes quiet".into()));
            }
        }
        Ok(())
    }

    fn exchange(&mut self, frame: &[u8], fc: u8) -> Result<Option<Vec<u8>>, Error> {
        let wait = self.ready_at.saturating_duration_since(Instant::now());
        if !wait.is_zero() {
            thread::sleep(wait);
        }
        // Serial §2.5.1.1 p.13: idle for t3.5 before a frame. After a link
        // error, idle for a whole response time-out: a late reply lands
        // here instead of in the next exchange.
        let quiet = if self.dirty {
            self.params.timeout
        } else {
            self.timing.t35
        };
        self.drain(quiet)?;
        self.dirty = false;
        self.line.send(frame)?;
        if self.params.unit == BROADCAST {
            self.ready_at = Instant::now() + self.params.turnaround;
            return Ok(None);
        }
        let deadline = Instant::now() + self.params.timeout;
        let max_gap = self.params.strict.then_some(self.timing.t15);
        let mut rx = Receiver::new(self.params.unit, fc, max_gap);
        let mut buf = [0u8; MAX_ADU];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(rx.give_up(self.params.timeout));
            }
            let n = self.line.recv(&mut buf, left)?;
            if n == 0 {
                continue;
            }
            let now = Instant::now();
            if let Some(pdu) = rx.push(&buf[..n], now) {
                self.ready_at = now + self.timing.t35;
                return Ok(Some(pdu));
            }
        }
    }
}

impl<L: Line> Transport for RtuClient<L> {
    fn request(&mut self, pdu: &[u8]) -> Result<Option<Vec<u8>>, Error> {
        let fc = *pdu
            .first()
            .ok_or_else(|| Error::Protocol("empty request".into()))?;
        if self.params.unit == BROADCAST
            && !matches!(
                fc,
                WRITE_SINGLE_COIL | WRITE_SINGLE_REGISTER | WRITE_MULTIPLE_COILS | WRITE_MULTIPLE_REGISTERS
            )
        {
            // Serial §2.1 p.7: broadcast requests are writes; nothing
            // would ever answer a read.
            return Err(Error::Point(format!(
                "unit 0 is broadcast: function {fc:#04x} is not a write, and nothing would reply"
            )));
        }
        let frame = encode(self.params.unit, pdu)?;
        let r = self.exchange(&frame, fc);
        if r.as_ref().is_err_and(Error::is_link) {
            self.dirty = true;
        }
        r
    }
}

/// The RTU driver: the shared Modbus driver over an RTU master.
pub type RtuDriver<L> = ModbusDriver<RtuClient<L>>;

impl<L: Line> ModbusDriver<RtuClient<L>> {
    #[must_use]
    pub fn rtu(line: L, params: Params) -> Self {
        Self {
            client: RtuClient::new(line, params),
        }
    }
}

/// Opens a `modbus-rtu` device: the endpoint is the serial port's path.
/// The port opens on the first request and reopens after it fails.
///
/// # Errors
/// A param that does not parse, or a build without serial support.
pub fn open(endpoint: &str, params: &BTreeMap<String, String>) -> Result<Box<dyn Driver + Send>, Error> {
    let p = Params::parse(params)?;
    #[cfg(all(unix, feature = "serial"))]
    {
        Ok(Box::new(ModbusDriver::rtu(
            serial::SerialLine::new(endpoint, p.settings),
            p,
        )))
    }
    #[cfg(not(all(unix, feature = "serial")))]
    {
        let _ = (endpoint, p);
        Err(Error::Io(io::Error::new(
            io::ErrorKind::Unsupported,
            "modbus-rtu: this build has no serial port support (helmsman-io's `serial` feature, unix)",
        )))
    }
}
