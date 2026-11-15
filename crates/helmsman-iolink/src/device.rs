//! A simulated Device, for tests and benches: the other end of
//! [`crate::master::Master`], built from the same clauses and none of the
//! Master's code beyond the codec. It answers on the page channel, moves
//! between its modes on `MasterCommand`s (IOL Figure 37 p.87, Table 57
//! p.108) and exchanges Process Data in OPERATE.

use crate::checksum;
use crate::mseq::{Channel, Error, Layout, MasterMessage, Reply};
use crate::page::{self, PAGE_1_LEN, Page1, command};
use crate::timing::{Rate, T_DSIO_US};

/// IOL Figure 37 p.87: the Device's modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Not communicating: before a wake-up, after a fallback.
    Sio,
    /// Woken, waiting for the test message at its rate.
    EstablishCom,
    Startup,
    Preoperate,
    Operate,
}

/// A Device with a fixed rate and Direct Parameter page.
#[derive(Debug)]
pub struct Device {
    rate: Rate,
    initial: [u8; PAGE_1_LEN],
    page: [u8; PAGE_1_LEN],
    mode: Mode,
    woke_at: u64,
    preoperate: Layout,
    operate: Layout,
    pd_in: [u8; 32],
    pd_out: [u8; 32],
    pd_out_valid: bool,
    pd_valid: bool,
    event: bool,
}

impl Device {
    /// # Errors
    /// As [`Page1::preoperate`] and [`Page1::operate`]: the page has to
    /// select M-sequence types the codec has.
    pub fn new(rate: Rate, page: Page1) -> Result<Self, Error> {
        Ok(Self {
            rate,
            initial: page.0,
            page: page.0,
            mode: Mode::Sio,
            woke_at: 0,
            preoperate: page.preoperate()?,
            operate: page.operate()?,
            pd_in: [0; 32],
            pd_out: [0; 32],
            pd_out_valid: false,
            pd_valid: true,
            event: false,
        })
    }

    #[must_use]
    pub const fn mode(&self) -> Mode {
        self.mode
    }

    /// The `MasterCycleTime` octet the Master wrote (IOL Table B.1).
    #[must_use]
    pub const fn master_cycle_time(&self) -> u8 {
        self.page[page::MASTER_CYCLE_TIME as usize]
    }

    /// The output Process Data, once the Master has declared it valid
    /// (`ProcessDataOutputOperate`, IOL Table B.2).
    #[must_use]
    pub fn outputs(&self) -> Option<&[u8]> {
        (self.mode == Mode::Operate && self.pd_out_valid).then(|| &self.pd_out[..self.operate.pd_out()])
    }

    /// The input Process Data to answer with; a shorter `data` leaves the
    /// rest as it was, a longer one is cut.
    pub fn set_inputs(&mut self, data: &[u8]) {
        let len = data.len().min(self.operate.pd_in());
        self.pd_in[..len].copy_from_slice(&data[..len]);
    }

    /// The PD status flag of every reply (IOL Table A.5).
    pub const fn set_pd_valid(&mut self, valid: bool) {
        self.pd_valid = valid;
    }

    /// The Event flag of every reply (IOL Table A.6).
    pub const fn set_event(&mut self, event: bool) {
        self.event = event;
    }

    /// Power off and on: SIO mode, the page as built.
    pub const fn power_cycle(&mut self) {
        self.mode = Mode::Sio;
        self.page = self.initial;
        self.pd_out_valid = false;
    }

    /// The wake-up current pulse (IOL Table 45 T1 p.87).
    pub const fn wake_up(&mut self, now: u64) {
        if matches!(self.mode, Mode::Sio | Mode::EstablishCom) {
            self.mode = Mode::EstablishCom;
            self.woke_at = now;
        }
    }

    /// A Master message sent at `rate`. Writes the reply into `out` and
    /// returns its length, or `None` when the Device stays silent: not
    /// woken, another rate, a damaged message (IOL §A.4.1.1 p.249:
    /// "Devices shall reject all data with detected errors and create no
    /// reaction"), a message of another M-sequence type.
    pub fn exchange(&mut self, now: u64, rate: Rate, message: &[u8], out: &mut [u8]) -> Option<usize> {
        // IOL Table 45 T10 p.88.
        if self.mode == Mode::EstablishCom && now.saturating_sub(self.woke_at) > T_DSIO_US {
            self.mode = Mode::Sio;
        }
        if rate != self.rate {
            return None;
        }
        let layout = match self.mode {
            Mode::Sio => return None,
            Mode::EstablishCom | Mode::Startup => Layout::TYPE_0,
            Mode::Preoperate => self.preoperate,
            Mode::Operate => self.operate,
        };
        let Ok(message) = MasterMessage::decode(layout, message) else {
            // IOL Table 45 T11 and T12 p.88, Table 47 T8 p.96: an intact
            // message of another type sends the Device back to STARTUP,
            // which is how a Master that started over finds it again.
            if matches!(self.mode, Mode::Preoperate | Mode::Operate) && other_type(layout, message) {
                self.leave(Mode::Startup);
            }
            return None;
        };
        let read = message.mc.read();
        let mut od = [0; 32];
        let mut next = if self.mode == Mode::EstablishCom {
            Mode::Startup
        } else {
            self.mode
        };
        if self.mode == Mode::Operate {
            self.pd_out[..layout.pd_out()].copy_from_slice(message.pd_out);
        }
        if message.mc.channel() == Channel::Page {
            let address = message.mc.address();
            if read {
                // IOL Table B.1 NOTE 1 and §B.1.1 p.258: the two command
                // octets and anything not implemented read as 0.
                if (page::MASTER_CYCLE_TIME..page::SYSTEM_COMMAND).contains(&address) {
                    od[0] = self.page[usize::from(address)];
                }
            } else if let Some(&value) = message.od.first() {
                match address {
                    page::MASTER_COMMAND => next = self.command(next, value),
                    // The octets IOL Table B.1 marks R/W.
                    page::MASTER_CYCLE_TIME | page::REVISION_ID | 0x09..=0x0B => {
                        self.page[usize::from(address)] = value;
                    }
                    _ => {}
                }
            }
        }
        let reply = Reply {
            od: &od[..if read { layout.od() } else { 0 }],
            pd_in: &self.pd_in[..layout.pd_in()],
            pd_valid: self.pd_valid,
            event: self.event,
        };
        let len = reply.encode(layout, read, out).ok()?;
        if next != self.mode {
            self.leave(next);
        }
        Some(len)
    }

    /// IOL Table 57 p.108 and Table 45 p.87-88: the mode after a
    /// `MasterCommand`. `DeviceOperate` from STARTUP is for legacy
    /// Masters only (T5) and is not taken.
    const fn command(&mut self, mode: Mode, value: u8) -> Mode {
        match (value, mode) {
            (command::FALLBACK, _) => Mode::Sio,
            (command::DEVICE_STARTUP, _) => Mode::Startup,
            (command::DEVICE_PREOPERATE, Mode::Startup) => Mode::Preoperate,
            (command::DEVICE_OPERATE, Mode::Preoperate) => Mode::Operate,
            (command::DEVICE_OPERATE, Mode::Operate) => {
                self.pd_out_valid = false;
                mode
            }
            (command::PROCESS_DATA_OUTPUT_OPERATE, Mode::Operate) => {
                self.pd_out_valid = true;
                mode
            }
            _ => mode,
        }
    }

    const fn leave(&mut self, next: Mode) {
        self.mode = next;
        self.pd_out_valid = false;
    }
}

/// An undamaged message whose check/type octet names another type than
/// `layout` (IOL Figure 44 p.95: "`TypeError` and not `ChecksumError`").
fn other_type(layout: Layout, message: &[u8]) -> bool {
    message.len() >= 2
        && message[1] >> 6 != layout.mtype()
        && checksum::of(message, 1) == message[1] & checksum::MASK
}
