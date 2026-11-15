//! One Master port, as a state machine without I/O: every call takes
//! `now` (microseconds on the caller's clock) and [`Master::poll`] says
//! what the line has to do next. The caller owns the transceiver: it
//! makes the wake-up pulse, sends the octets at the rate given, collects
//! the reply until the timeout and hands back whatever arrived.
//!
//! The port looks for a Device (wake-up, then a test message at COM3,
//! COM2 and COM1; IOL §7.3.2.2 p.80-82), reads its Direct Parameter page,
//! takes it through PREOPERATE to OPERATE (Figure 82 p.155) and exchanges
//! Process Data every cycle. A failed message is sent again twice and
//! then the Device counts as lost (Table 46 p.92-95), the inputs go
//! invalid and the search starts over.
//!
//! Two choices here are ours, where the specification leaves the port's
//! policy to System Management: a port that found nobody, lost its Device
//! or refused it keeps looking, one retry sequence every `T_SD`; and the
//! compatibility checks all run once the page has been read.
//!
//! Not here: ISDUs, Events (the flag is kept, the Event is not fetched),
//! the interleaved mode, legacy Devices, SIO mode and the fallback to it.

use crate::mseq::{Error, Layout, MAX_MESSAGE, MasterMessage, Mc, Reply};
use crate::page::{self, PAGE_1_LEN, Page1, command, cycle_time, cycle_time_octet};
use crate::timing::{
    MAX_RETRY, Rate, T_DMT_BITS, T_DWU_US, T_REN_US, T_SD_US, T_WU_US, WAKE_UPS, m_sequence_bits,
};

/// A read of `MinCycleTime`, the test message (IOL Table 46 T1 p.93: "MC =
/// 0xA2").
const TEST: u8 = 0xA2;
/// A read on the ISDU channel with flow control IDLE (IOL Table 52 p.103),
/// "the default message if there are no On-request Data pending" (§7.3.5
/// NOTE p.101).
const IDLE: u8 = 0xF1;
/// Page channel, read (IOL Figure A.1).
const PAGE_READ: u8 = 0xA0;
/// Page channel, write.
const PAGE_WRITE: u8 = 0x20;

/// Which Device a port is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub vendor: u16,
    pub device: u32,
}

/// What the port is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Config {
    /// The cycle time wanted, in microseconds. The port never runs
    /// faster than the Device's `MinCycleTime` or than the M-sequence
    /// takes, so 0 is "as fast as the Device allows".
    pub cycle_us: u32,
    /// When set, a Device with another vendor or device id is refused.
    pub expect: Option<Identity>,
}

/// Where the port is (IOL Figure 35 p.84).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// No Device answers: waking up, probing the rates, or waiting.
    Searching,
    /// A Device answered; its page is being read.
    Startup,
    /// The Device is in PREOPERATE and is being given its cycle time.
    Preoperate,
    /// Cyclic Process Data.
    Operate,
}

/// Why the Device on the port was not taken to OPERATE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// Its `RevisionID` is not 1.1 (IOL §B.1.5 p.261).
    Revision(u8),
    /// It is not the Device the port was configured for.
    Identity(Identity),
    /// Its M-sequence capability and Process Data lengths select no type
    /// this port speaks.
    Layout(Error),
}

/// What the line does next.
#[derive(Debug, PartialEq, Eq)]
pub enum Step<'a> {
    /// Make the wake-up current pulse (IOL §5.3.3.3 p.60), then poll
    /// again.
    WakeUp,
    /// Send `send` at `rate`, collect up to `reply` octets for at most
    /// `timeout_us` from the start of sending, then call
    /// [`Master::reply`] with what arrived.
    Exchange {
        rate: Rate,
        send: &'a [u8],
        reply: usize,
        timeout_us: u64,
    },
    /// Nothing before `until`.
    Idle { until: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Rest,
    Probe(Rate),
    Startup,
    Operate,
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    read: bool,
    deadline: u64,
}

enum Op {
    Read(u8),
    Write(u8, u8),
}

/// The acyclic recovery time `t_initcyc` in bit times (IOL Tables A.7
/// and A.8 p.245), between the starts of two requests.
const fn recovery_bits(layout: Layout) -> usize {
    match layout.od() {
        8 => 210,
        32 => 550,
        _ => 100,
    }
}

/// One Master port.
// Four flags that are facts of their own (outputs valid, the Device told
// so, inputs valid, an Event waiting), not one state in pieces.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug)]
pub struct Master {
    config: Config,
    phase: Phase,
    wake_ups: u8,
    rate: Rate,
    layout: Layout,
    preoperate: Layout,
    operate: Layout,
    page: [u8; PAGE_1_LEN],
    step: u8,
    retries: u8,
    next_at: u64,
    sent_at: u64,
    pending: Option<Pending>,
    tx: [u8; MAX_MESSAGE],
    out: [u8; 32],
    out_valid: bool,
    told_valid: bool,
    telling: Option<bool>,
    inp: [u8; 32],
    inp_valid: bool,
    event: bool,
    cycle_octet: u8,
    cycle_us: u64,
    fault: Option<Fault>,
    losses: u32,
    failures: u32,
}

impl Master {
    #[must_use]
    pub const fn new(config: Config) -> Self {
        Self {
            config,
            phase: Phase::Rest,
            wake_ups: 0,
            rate: Rate::Com3,
            layout: Layout::TYPE_0,
            preoperate: Layout::TYPE_0,
            operate: Layout::TYPE_0,
            page: [0; PAGE_1_LEN],
            step: 0,
            retries: 0,
            next_at: 0,
            sent_at: 0,
            pending: None,
            tx: [0; MAX_MESSAGE],
            out: [0; 32],
            out_valid: false,
            told_valid: false,
            telling: None,
            inp: [0; 32],
            inp_valid: false,
            event: false,
            cycle_octet: 0,
            cycle_us: 0,
            fault: None,
            losses: 0,
            failures: 0,
        }
    }

    #[must_use]
    pub const fn state(&self) -> State {
        match self.phase {
            Phase::Rest | Phase::Probe(_) => State::Searching,
            Phase::Startup if self.step > 13 => State::Preoperate,
            Phase::Startup => State::Startup,
            Phase::Operate => State::Operate,
        }
    }

    /// Why the last Device seen was refused; cleared when one reaches
    /// OPERATE.
    #[must_use]
    pub const fn fault(&self) -> Option<Fault> {
        self.fault
    }

    /// The rate the Device answered at, once one has.
    #[must_use]
    pub const fn rate(&self) -> Option<Rate> {
        match self.phase {
            Phase::Rest | Phase::Probe(_) => None,
            Phase::Startup | Phase::Operate => Some(self.rate),
        }
    }

    /// The Device's Direct Parameter page 1, once it has been read and
    /// accepted (addresses 0x00, 0x01, 0x0E and 0x0F are not read).
    #[must_use]
    pub const fn page(&self) -> Option<Page1> {
        if matches!(self.state(), State::Preoperate | State::Operate) {
            Some(Page1(self.page))
        } else {
            None
        }
    }

    /// The cycle time in use, in microseconds; 0 before a Device is
    /// accepted. What the Device was told is this rounded into IOL Table
    /// B.3.
    #[must_use]
    pub const fn cycle_us(&self) -> u64 {
        self.cycle_us
    }

    /// The Device's input Process Data of the last good cycle. `None`
    /// outside OPERATE, before the first cycle, and while the Device
    /// flags its Process Data invalid (IOL Table A.5).
    #[must_use]
    pub fn inputs(&self) -> Option<&[u8]> {
        (self.phase == Phase::Operate && self.inp_valid).then(|| &self.inp[..self.operate.pd_in()])
    }

    /// The output Process Data to send from the next cycle on; `None`
    /// tells the Device its outputs are invalid (IOL Table 55 p.106) and
    /// sends zeros. Kept across a lost Device and told again.
    ///
    /// # Errors
    /// [`Error::Length`] when `data` is not as long as the accepted
    /// Device's output Process Data (0 before one is accepted).
    pub fn set_outputs(&mut self, data: Option<&[u8]>) -> Result<(), Error> {
        let want = self.operate.pd_out();
        let Some(data) = data else {
            self.out = [0; 32];
            self.out_valid = false;
            return Ok(());
        };
        if data.len() != want {
            return Err(Error::Length {
                want,
                got: data.len(),
            });
        }
        self.out[..want].copy_from_slice(data);
        self.out_valid = true;
        Ok(())
    }

    /// The Device has an Event waiting (IOL Table A.6). Nothing fetches
    /// it yet.
    #[must_use]
    pub const fn event(&self) -> bool {
        self.event
    }

    /// How often a Device was lost after its retries.
    #[must_use]
    pub const fn losses(&self) -> u32 {
        self.losses
    }

    /// How many messages to a found Device got no good reply.
    #[must_use]
    pub const fn failures(&self) -> u32 {
        self.failures
    }

    /// What the line does at `now`. Call it again after a
    /// [`Step::WakeUp`], after [`Master::reply`], and when a
    /// [`Step::Idle`] has run out. Polling while a reply is still owed
    /// waits for its timeout and then counts the message as unanswered.
    pub fn poll(&mut self, now: u64) -> Step<'_> {
        if let Some(pending) = self.pending {
            if now < pending.deadline {
                return Step::Idle {
                    until: pending.deadline,
                };
            }
            self.pending = None;
            self.failed(now);
        }
        if now < self.next_at {
            return Step::Idle { until: self.next_at };
        }
        match self.phase {
            Phase::Rest => {
                self.wake_ups += 1;
                self.phase = Phase::Probe(Rate::Com3);
                self.next_at = now + T_WU_US + T_REN_US + Rate::Com3.micros(T_DMT_BITS);
                Step::WakeUp
            }
            Phase::Probe(rate) => {
                self.rate = rate;
                self.layout = Layout::TYPE_0;
                self.exchange(now, TEST, None)
            }
            Phase::Startup => match self.op() {
                Op::Read(address) => self.exchange(now, PAGE_READ | address, None),
                Op::Write(address, value) => self.exchange(now, PAGE_WRITE | address, Some(value)),
            },
            Phase::Operate => {
                // IOL Table 56 T3 p.107: the state of the outputs goes to
                // the Device as a MasterCommand, once per change.
                self.telling = (self.operate.pd_out() > 0 && self.out_valid != self.told_valid)
                    .then_some(self.out_valid);
                match self.telling {
                    Some(true) => self.exchange(
                        now,
                        PAGE_WRITE | page::MASTER_COMMAND,
                        Some(command::PROCESS_DATA_OUTPUT_OPERATE),
                    ),
                    Some(false) => self.exchange(
                        now,
                        PAGE_WRITE | page::MASTER_COMMAND,
                        Some(command::DEVICE_OPERATE),
                    ),
                    None => self.exchange(now, IDLE, None),
                }
            }
        }
    }

    /// The octets that arrived for the last [`Step::Exchange`], all of
    /// them or fewer or none. Ignored when no reply is owed.
    pub fn reply(&mut self, now: u64, octets: &[u8]) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        match Reply::decode(self.layout, pending.read, octets) {
            Ok(reply) => {
                self.retries = 0;
                self.accept(now, &reply);
            }
            Err(_) => self.failed(now),
        }
    }

    /// The startup of IOL Figure 82 p.155 and Table 86 p.140, one page
    /// access per step.
    const fn op(&self) -> Op {
        match self.step {
            // The communication parameters, 0x02 to 0x06.
            0..=4 => Op::Read(page::MIN_CYCLE_TIME + self.step),
            5 => Op::Write(page::MASTER_COMMAND, command::MASTER_IDENT),
            // The identification parameters, 0x07 to 0x0D.
            6..=12 => Op::Read(page::VENDOR_ID + self.step - 6),
            13 => Op::Write(page::MASTER_COMMAND, command::DEVICE_PREOPERATE),
            14 => Op::Write(page::MASTER_CYCLE_TIME, self.cycle_octet),
            _ => Op::Write(page::MASTER_COMMAND, command::DEVICE_OPERATE),
        }
    }

    fn exchange(&mut self, now: u64, mc: u8, value: Option<u8>) -> Step<'_> {
        let mc = Mc::from_octet(mc);
        let read = mc.read();
        let layout = self.layout;
        // IOL §A.2.3 p.242: with several octets of On-request Data only
        // the first is relevant on the page channel, the rest are 0x00.
        let mut od = [0; 32];
        od[0] = value.unwrap_or(0);
        let message = MasterMessage {
            mc,
            pd_out: &self.out[..layout.pd_out()],
            od: &od[..if read { 0 } else { layout.od() }],
        };
        let Ok(len) = message.encode(layout, &mut self.tx) else {
            // Not reachable: the parts are cut to the layout and the
            // buffer holds the longest message.
            self.rest(now + T_SD_US);
            return Step::Idle { until: self.next_at };
        };
        let reply = layout.reply_len(read);
        let timeout_us = self.rate.micros(m_sequence_bits(len, reply));
        self.sent_at = now;
        self.pending = Some(Pending {
            read,
            deadline: now + timeout_us,
        });
        self.next_at = now
            + match self.phase {
                Phase::Operate => self.cycle_us,
                _ => self.rate.micros(recovery_bits(layout)),
            };
        Step::Exchange {
            rate: self.rate,
            send: &self.tx[..len],
            reply,
            timeout_us,
        }
    }

    fn accept(&mut self, now: u64, reply: &Reply<'_>) {
        match self.phase {
            Phase::Rest => {}
            Phase::Probe(_) => {
                self.page = [0; PAGE_1_LEN];
                self.page[usize::from(page::MIN_CYCLE_TIME)] = reply.od.first().copied().unwrap_or(0);
                self.wake_ups = 0;
                self.step = 1;
                self.phase = Phase::Startup;
            }
            Phase::Startup => self.advance(now, reply),
            Phase::Operate => {
                let len = self.operate.pd_in();
                self.inp[..len].copy_from_slice(reply.pd_in);
                self.inp_valid = reply.pd_valid;
                self.event = reply.event;
                if let Some(valid) = self.telling.take() {
                    self.told_valid = valid;
                }
            }
        }
    }

    fn advance(&mut self, now: u64, reply: &Reply<'_>) {
        if let Op::Read(address) = self.op() {
            self.page[usize::from(address)] = reply.od.first().copied().unwrap_or(0);
        }
        match self.step {
            12 => {
                if let Err(fault) = self.check() {
                    self.fault = Some(fault);
                    self.rest(now + T_SD_US);
                    return;
                }
            }
            13 => self.switch(self.preoperate),
            15 => {
                self.switch(self.operate);
                self.phase = Phase::Operate;
                self.fault = None;
                self.told_valid = false;
                self.inp_valid = false;
                self.event = false;
                return;
            }
            _ => {}
        }
        self.step += 1;
    }

    /// The Device changed mode with the message just acknowledged: the
    /// next one has the new type, no sooner than that type's recovery
    /// time after the last.
    fn switch(&mut self, layout: Layout) {
        self.layout = layout;
        self.next_at = self
            .next_at
            .max(self.sent_at + self.rate.micros(recovery_bits(layout)));
    }

    /// The checks of IOL Table 86 p.140 on the page just read, and the
    /// M-sequence types and cycle time that follow from it.
    fn check(&mut self) -> Result<(), Fault> {
        let page = Page1(self.page);
        if page.revision() != page::REVISION_1_1 {
            return Err(Fault::Revision(page.revision()));
        }
        let found = Identity {
            vendor: page.vendor_id(),
            device: page.device_id(),
        };
        if self.config.expect.is_some_and(|expect| expect != found) {
            return Err(Fault::Identity(found));
        }
        let preoperate = page.preoperate().map_err(Fault::Layout)?;
        let operate = page.operate().map_err(Fault::Layout)?;
        if operate != self.operate {
            // Another Device than last time: the outputs held are not its.
            self.out = [0; 32];
            self.out_valid = false;
        }
        self.preoperate = preoperate;
        self.operate = operate;
        // IOL §B.1.3 p.260: never below the Device's minimum, and never
        // below the worst-case M-sequence time when the Device states
        // none or a shorter one.
        let bits = m_sequence_bits(operate.master_len(true), operate.reply_len(true)).max(m_sequence_bits(
            operate.master_len(false),
            operate.reply_len(false),
        ));
        let floor = self.rate.micros(bits);
        let min = u64::from(page.min_cycle_time().unwrap_or(0)) * 100;
        let want = floor.max(min).max(u64::from(self.config.cycle_us));
        self.cycle_octet = cycle_time_octet(u16::try_from(want.div_ceil(100)).unwrap_or(u16::MAX));
        // The longest M-sequences at COM1 outlast the table's 132,8 ms.
        self.cycle_us = (u64::from(cycle_time(self.cycle_octet).unwrap_or(0)) * 100).max(floor);
        Ok(())
    }

    fn failed(&mut self, now: u64) {
        match self.phase {
            Phase::Rest => {}
            // IOL Figure 36 p.84: the next rate after `T_DMT`, then the
            // next wake-up after `T_DWU`, then a pause of `T_SD`.
            Phase::Probe(rate) => match rate.lower() {
                Some(lower) => {
                    self.phase = Phase::Probe(lower);
                    self.next_at = now + lower.micros(T_DMT_BITS);
                }
                None if self.wake_ups < WAKE_UPS => {
                    self.phase = Phase::Rest;
                    self.next_at = now + T_DWU_US;
                }
                None => self.rest(now + T_SD_US),
            },
            // IOL Table 46 p.93-95: sent again after the recovery or the
            // cycle time (set when it was sent), twice, then COMLOST.
            Phase::Startup | Phase::Operate => {
                self.failures = self.failures.saturating_add(1);
                self.retries += 1;
                if self.retries > MAX_RETRY {
                    self.losses = self.losses.saturating_add(1);
                    self.rest(now + T_DWU_US);
                }
            }
        }
    }

    /// Back to looking for a Device, from `at`.
    fn rest(&mut self, at: u64) {
        self.phase = Phase::Rest;
        self.pending = None;
        self.wake_ups = 0;
        self.retries = 0;
        self.step = 0;
        self.layout = Layout::TYPE_0;
        self.inp_valid = false;
        self.told_valid = false;
        self.telling = None;
        self.next_at = at;
    }
}
