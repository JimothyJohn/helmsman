//! The kernel side of [`super::Board`] on Linux: GPIO through the
//! character device's uAPI v2 (`/dev/gpiochipN`, `GPIO_V2_GET_LINE_IOCTL`,
//! via the `gpiocdev` crate) and I2C through `/dev/i2c-N` (`I2C_RDWR`, via
//! the `i2cdev` crate). Kept thin on purpose: every rule about channels
//! lives in [`super`], tested without hardware.
//!
//! Each line is requested on first use, consumer `helmsman`, and held for
//! the life of the process (the kernel releases it when the request's fd
//! closes). An output starts off.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::io;
use std::path::{Path, PathBuf};

use gpiocdev::line::Value as Level;
use gpiocdev::{Chip, Request};
use i2cdev::core::{I2CMessage, I2CTransfer};
use i2cdev::linux::{LinuxI2CBus, LinuxI2CMessage};

use super::{Board, Gpio, I2c};
use crate::driver::Error;

const CONSUMER: &str = "helmsman";

/// Lines on one GPIO chip.
pub struct ChipGpio {
    path: PathBuf,
    lines: BTreeMap<u32, (Request, bool)>,
}

impl ChipGpio {
    /// Checks the chip is there (so a wrong `--gpio-chip` fails at start).
    ///
    /// # Errors
    /// The chip does not open.
    pub fn open(path: &Path) -> io::Result<Self> {
        let chip = Chip::from_path(path).map_err(|e| io::Error::other(format!("{}: {e}", path.display())))?;
        drop(chip);
        Ok(Self {
            path: path.into(),
            lines: BTreeMap::new(),
        })
    }

    fn line(&mut self, line: u32, output: bool) -> Result<&Request, Error> {
        if self.lines.get(&line).is_none_or(|(_, o)| *o != output) {
            // Release any request in the other direction first.
            self.lines.remove(&line);
            let mut b = Request::builder();
            b.on_chip(&self.path).with_consumer(CONSUMER).with_line(line);
            if output {
                b.as_output(Level::Inactive);
            } else {
                b.as_input();
            }
            let req = b
                .request()
                .map_err(|e| Error::Point(format!("{} line {line}: {e}", self.path.display())))?;
            self.lines.insert(line, (req, output));
        }
        Ok(&self.lines.get(&line).expect("inserted above").0)
    }
}

impl Gpio for ChipGpio {
    fn get(&mut self, line: u32) -> Result<bool, Error> {
        // An output reads back as its own request; anything else as input.
        let output = self.lines.get(&line).is_some_and(|(_, o)| *o);
        let path = self.path.display().to_string();
        let req = self.line(line, output)?;
        req.value(line)
            .map(|v| v == Level::Active)
            .map_err(|e| Error::Point(format!("{path} line {line}: {e}")))
    }

    fn set(&mut self, line: u32, on: bool) -> Result<(), Error> {
        let path = self.path.display().to_string();
        let req = self.line(line, true)?;
        let v = if on { Level::Active } else { Level::Inactive };
        req.set_value(line, v)
            .map_err(|e| Error::Point(format!("{path} line {line}: {e}")))
    }
}

/// `/dev/i2c-N`, each bus opened on first use.
#[derive(Default)]
pub struct DevI2c {
    buses: BTreeMap<u8, LinuxI2CBus>,
}

impl I2c for DevI2c {
    fn read_register(&mut self, bus: u8, addr: u8, reg: u8, buf: &mut [u8]) -> Result<(), Error> {
        let path = format!("/dev/i2c-{bus}");
        let b = match self.buses.entry(bus) {
            Entry::Occupied(o) => o.into_mut(),
            Entry::Vacant(v) => {
                v.insert(LinuxI2CBus::new(&path).map_err(|e| Error::Point(format!("{path}: {e}")))?)
            }
        };
        let reg = [reg];
        let mut msgs = [
            LinuxI2CMessage::write(&reg).with_address(u16::from(addr)),
            LinuxI2CMessage::read(buf).with_address(u16::from(addr)),
        ];
        match b.transfer(&mut msgs) {
            Ok(2) => Ok(()),
            Ok(n) => Err(Error::Point(format!(
                "{path} {addr:#04x}: {n} of 2 messages went through"
            ))),
            Err(e) => {
                // Reopen next time: the adapter may have gone away.
                self.buses.remove(&bus);
                Err(Error::Point(format!("{path} {addr:#04x}: {e}")))
            }
        }
    }
}

/// The Pi's channels: GPIO on `chip`, I2C on `/dev/i2c-N`.
///
/// # Errors
/// The chip does not open.
pub fn open(chip: &Path) -> io::Result<Board<ChipGpio, DevI2c>> {
    Ok(Board::new(ChipGpio::open(chip)?, DevI2c::default()))
}
