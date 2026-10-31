//! The box's own channels: their address syntax, and a [`Driver`] over the
//! board's GPIO lines and I2C buses. [`crate::local::Channels`] is the same
//! syntax in memory (a dev machine); [`linux`] is the kernel side on a Pi
//! (Linux and the `gpio` feature only).
//!
//! Channels:
//!
//! - `do:N`: a digital output, GPIO line N of the chip the box was given
//!   (through a 24 V output stage on a real install);
//! - `di:N`: a digital input, GPIO line N;
//! - `i2c:BUS:ADDR:REG[:FMT]`: a read-only integer from a sensor on
//!   `/dev/i2c-BUS` (0..=255, Linux names every I2C adapter `i2c-%d`): the
//!   register REG (0..=255) of the device at 7-bit address ADDR (`0x08` to
//!   `0x77`, the range `i2cdetect` scans by default), as `0x` hex or
//!   decimal. FMT is `u8` (default), `i8`, or `u16` / `i16` (two bytes, most
//!   significant first) or `u16le` / `i16le` (least significant first). A
//!   read writes the register number, then reads the value without a stop
//!   in between (`I2C_RDWR`).
//!
//! Errors: a GPIO chip that won't open stops the box at start (it is
//! checked when the backend is built); a line that won't request, or an
//! I2C device that doesn't answer, is a point error, so one unplugged
//! sensor doesn't take the box's other channels down with it.

use std::collections::BTreeMap;

use crate::driver::{Driver, Error, PointInfo};
use crate::model::{Ty, Value};

#[cfg(all(target_os = "linux", feature = "gpio"))]
pub mod linux;

/// The lowest and highest 7-bit addresses accepted.
pub const I2C_ADDR_MIN: u8 = 0x08;
pub const I2C_ADDR_MAX: u8 = 0x77;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum I2cFormat {
    U8,
    I8,
    U16,
    I16,
    U16Le,
    I16Le,
}

impl I2cFormat {
    /// Bytes read.
    #[must_use]
    pub const fn bytes(self) -> usize {
        match self {
            Self::U8 | Self::I8 => 1,
            _ => 2,
        }
    }

    /// The value from exactly [`Self::bytes`] bytes, as read.
    ///
    /// # Panics
    /// `bytes` is shorter than [`Self::bytes`].
    #[must_use]
    pub fn decode(self, bytes: &[u8]) -> i64 {
        let be = || u16::from_be_bytes([bytes[0], bytes[1]]);
        let le = || u16::from_le_bytes([bytes[0], bytes[1]]);
        match self {
            Self::U8 => i64::from(bytes[0]),
            Self::I8 => i64::from(bytes[0].cast_signed()),
            Self::U16 => i64::from(be()),
            Self::I16 => i64::from(be().cast_signed()),
            Self::U16Le => i64::from(le()),
            Self::I16Le => i64::from(le().cast_signed()),
        }
    }
}

/// One register of one I2C device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct I2cPoint {
    pub bus: u8,
    pub addr: u8,
    pub reg: u8,
    pub format: I2cFormat,
}

/// A parsed box channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Out(u16),
    In(u16),
    I2c(I2cPoint),
}

/// Decimal digits only (no sign, no spaces), at most `max`.
fn decimal(s: &str, max: u32) -> Option<u32> {
    if s.is_empty() || s.len() > 10 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok().filter(|n| *n <= max)
}

/// `0x` + 1-2 hex digits, or decimal; at most 255.
fn byte(s: &str) -> Option<u8> {
    match s.strip_prefix("0x") {
        Some(h) if (1..=2).contains(&h.len()) && h.bytes().all(|b| b.is_ascii_hexdigit()) => {
            u8::from_str_radix(h, 16).ok()
        }
        Some(_) => None,
        None => decimal(s, 255).and_then(|n| u8::try_from(n).ok()),
    }
}

impl Channel {
    /// Parses a channel; see the module docs.
    ///
    /// # Errors
    /// [`Error::Address`] for anything else.
    pub fn parse(s: &str) -> Result<Self, Error> {
        let bad = || Error::Address(s.into());
        let parts: Vec<&str> = s.split(':').collect();
        let n16 = |t: &str| decimal(t, u32::from(u16::MAX)).and_then(|n| u16::try_from(n).ok());
        match parts.as_slice() {
            ["do", n] => n16(n).map(Self::Out).ok_or_else(bad),
            ["di", n] => n16(n).map(Self::In).ok_or_else(bad),
            ["i2c", bus, addr, reg, fmt @ ..] => {
                let format = match fmt {
                    [] | ["u8"] => I2cFormat::U8,
                    ["i8"] => I2cFormat::I8,
                    ["u16"] => I2cFormat::U16,
                    ["i16"] => I2cFormat::I16,
                    ["u16le"] => I2cFormat::U16Le,
                    ["i16le"] => I2cFormat::I16Le,
                    _ => return Err(bad()),
                };
                let bus = decimal(bus, 255)
                    .and_then(|n| u8::try_from(n).ok())
                    .ok_or_else(bad)?;
                let addr = byte(addr)
                    .filter(|a| (I2C_ADDR_MIN..=I2C_ADDR_MAX).contains(a))
                    .ok_or_else(bad)?;
                let reg = byte(reg).ok_or_else(bad)?;
                Ok(Self::I2c(I2cPoint {
                    bus,
                    addr,
                    reg,
                    format,
                }))
            }
            _ => Err(bad()),
        }
    }

    #[must_use]
    pub const fn info(self) -> PointInfo {
        match self {
            Self::Out(_) => PointInfo {
                ty: Ty::Bool,
                writable: true,
            },
            Self::In(_) => PointInfo {
                ty: Ty::Bool,
                writable: false,
            },
            Self::I2c(_) => PointInfo {
                ty: Ty::Int,
                writable: false,
            },
        }
    }
}

/// GPIO lines, by offset on one chip.
pub trait Gpio: Send {
    /// Reads a line, requesting it as an input first if need be.
    ///
    /// # Errors
    /// [`Error::Point`] for a line that can't be had, [`Error::Io`] when the
    /// chip itself is gone.
    fn get(&mut self, line: u32) -> Result<bool, Error>;

    /// Drives a line, requesting it as an output (initially off) first if
    /// need be.
    ///
    /// # Errors
    /// As [`Gpio::get`].
    fn set(&mut self, line: u32, on: bool) -> Result<(), Error>;
}

/// I2C buses, by number.
pub trait I2c: Send {
    /// Writes `reg` to the device at `addr` on bus `bus`, then reads
    /// `buf.len()` bytes, with no stop between.
    ///
    /// # Errors
    /// [`Error::Point`]: the bus or the device didn't answer.
    fn read_register(&mut self, bus: u8, addr: u8, reg: u8, buf: &mut [u8]) -> Result<(), Error>;
}

/// The box's channels over a [`Gpio`] and an [`I2c`]. A line serves as
/// `di:N` or as `do:N`, not both: they are one pin.
pub struct Board<G, I> {
    gpio: G,
    i2c: I,
    /// Lines in use: `true` for an output.
    used: BTreeMap<u16, bool>,
}

impl<G: Gpio, I: I2c> Board<G, I> {
    pub fn new(gpio: G, i2c: I) -> Self {
        Self {
            gpio,
            i2c,
            used: BTreeMap::new(),
        }
    }

    fn claim(&mut self, n: u16, output: bool) -> Result<(), Error> {
        match self.used.get(&n) {
            Some(&o) if o != output => {
                let (was, is) = if o { ("do", "di") } else { ("di", "do") };
                Err(Error::Point(format!(
                    "GPIO line {n} is already {was}:{n}; it can't also be {is}:{n}"
                )))
            }
            _ => {
                self.used.insert(n, output);
                Ok(())
            }
        }
    }

    fn read_one(&mut self, c: Channel) -> Result<Value, Error> {
        match c {
            Channel::In(n) => {
                self.claim(n, false)?;
                self.gpio.get(u32::from(n)).map(Value::Bool)
            }
            Channel::Out(n) => {
                self.claim(n, true)?;
                self.gpio.get(u32::from(n)).map(Value::Bool)
            }
            Channel::I2c(p) => {
                let mut buf = [0u8; 2];
                let buf = &mut buf[..p.format.bytes()];
                self.i2c.read_register(p.bus, p.addr, p.reg, buf)?;
                Ok(Value::Int(p.format.decode(buf)))
            }
        }
    }
}

impl<G: Gpio, I: I2c> Driver for Board<G, I> {
    fn parse(&self, addr: &str) -> Result<PointInfo, Error> {
        Channel::parse(addr).map(Channel::info)
    }

    fn read(&mut self, addrs: &[&str]) -> Result<Vec<Value>, Error> {
        let chans = addrs
            .iter()
            .map(|a| Channel::parse(a))
            .collect::<Result<Vec<_>, _>>()?;
        chans.into_iter().map(|c| self.read_one(c)).collect()
    }

    fn write(&mut self, addr: &str, value: Value) -> Result<(), Error> {
        let c = Channel::parse(addr)?;
        let Channel::Out(n) = c else {
            return Err(Error::ReadOnly(addr.into()));
        };
        let Value::Bool(on) = value else {
            return Err(Error::Type {
                want: Ty::Bool,
                got: value.ty(),
            });
        };
        self.claim(n, true)?;
        self.gpio.set(u32::from(n), on)
    }
}
