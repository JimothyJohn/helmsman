//! An IO controller for one device, as a state machine without I/O.
//!
//! The owner hands it what arrived (`on_frame` for raw PROFINET frames,
//! `on_udp` for datagrams at [`crate::rpc::PORT`]) and the time
//! (`poll`), and sends what it puts in `out`. Every call takes `now` in
//! microseconds on any monotonic clock.
//!
//! Startup, as a device's stack sees it (p-net `pf_cmrpc.c`, `pf_cmdev.c`):
//!
//! 1. **Identify** the station by name over DCP; give it its IP address
//!    with a DCP Set when one is configured and the station's differs.
//! 2. **Connect**: one RPC carrying the AR, an input and an output IOCR,
//!    the alarm CR and the expected modules. The response names the
//!    output frame id and what differs from the expectation. Cyclic
//!    output frames start here.
//! 3. **Write** each parameter record, then **`PrmEnd`**.
//! 4. The device calls back with **"application ready"**; the answer
//!    completes the startup, and inputs are served once a valid frame
//!    has arrived.
//!
//! Then cyclic data both ways, alarms acknowledged as they come, and a
//! watchdog: no valid input frame for the data hold time ends the AR and
//! the startup begins again after a pause.
//!
//! Not done here (docs/ARCHITECTURE.md lists them): RPC fragmentation (a
//! configuration whose Connect exceeds one datagram is refused when the
//! controller is built), record reads, RT classes 2 and 3, more than one
//! AR per device.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::alarm::{self, Notification, Rta};
use crate::block::{
    self, AlarmCrReq, ArReq, ConnectReq, ConnectRes, Control, IocrReq, ModuleDiff, WriteReq, WriteRes,
};
use crate::codec::{DecodeError, Order};
use crate::dcp::{self, Ip, Station};
use crate::eth::{self, Mac, Vlan};
use crate::rpc::{self, Header, Status, Uuid};
use crate::rt::{self, Layout, Module};

/// How often an unanswered Identify is repeated.
const IDENTIFY_PERIOD_US: u64 = 1_000_000;
/// How long a DCP Set or an RPC waits before it is sent again, and how
/// many times it is sent in all.
const REQUEST_TIMEOUT_US: u64 = 2_000_000;
const REQUEST_TRIES: u8 = 3;
/// How long the device has to say "application ready" after `PrmEnd`.
const READY_TIMEOUT_US: u64 = 30_000_000;
/// The pause before a failed startup is tried again.
const BACKOFF_US: u64 = 2_000_000;
/// The controller's alarm reference.
const ALARM_REF: u16 = 1;
/// Alarm transport: 100 ms per try, three tries (the smallest values a
/// device takes, p-net `pf_cmdev.c`).
const RTA_TIMEOUT_FACTOR: u16 = 1;
const RTA_RETRIES: u16 = 3;
const RTA_TIMEOUT_US: u64 = 100_000;

/// What the controller is on the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Local {
    pub mac: Mac,
    /// The controller's own station name, sent in the Connect.
    pub station_name: String,
    /// The controller's own ids, the tail of its object UUID.
    pub vendor_id: u16,
    pub device_id: u16,
}

/// An IP address to give the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Address {
    pub ip: Ip,
    pub mask: Ip,
    pub gateway: Ip,
}

/// The device as the controller expects it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceConfig {
    pub station_name: String,
    /// Set over DCP (until the device's next power cycle) when the
    /// station's own address differs. `None` uses whatever it has.
    pub address: Option<Address>,
    /// When set, a station with other ids is refused.
    pub vendor_id: Option<u16>,
    pub device_id: Option<u16>,
    /// The device instance in its object UUID, used when the station's
    /// Identify response does not say (GSDML `ObjectUUID_LocalIndex`).
    pub instance: u16,
    pub modules: Vec<Module>,
    /// The base clock in units of 31.25 us; 32 (1 ms) unless the device
    /// asks for another.
    pub send_clock_factor: u16,
    /// A frame every this many base clocks: 8 is every 8 ms.
    pub reduction_ratio: u16,
    /// Missed cycles after which either side gives up.
    pub data_hold_factor: u16,
    /// The advanced startup mode (AR property bit 30) or the legacy one.
    pub advanced_startup: bool,
    /// Whether cyclic frames carry an 802.1Q priority tag.
    pub vlan: bool,
    /// The input frame id, in 0xC000..=0xF7FF; unique among the devices
    /// of one controller.
    pub input_frame_id: u16,
}

impl DeviceConfig {
    /// A device by name and modules, with the usual timing: a frame every
    /// 8 ms, three missed cycles tolerated, tagged frames, legacy startup.
    #[must_use]
    pub fn new(station_name: &str, modules: Vec<Module>) -> Self {
        Self {
            station_name: station_name.into(),
            address: None,
            vendor_id: None,
            device_id: None,
            instance: 1,
            modules,
            send_clock_factor: 32,
            reduction_ratio: 8,
            data_hold_factor: 3,
            advanced_startup: false,
            vlan: true,
            input_frame_id: eth::FRAME_ID_RTC1_FIRST,
        }
    }
}

/// Something to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    /// A complete Ethernet frame.
    Frame(Vec<u8>),
    /// A datagram from the controller's [`rpc::PORT`] socket.
    Udp { ip: Ip, port: u16, data: Vec<u8> },
}

/// Where the startup is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Looking for the station by name.
    Identifying,
    /// Giving the station its IP address.
    Addressing,
    /// Connect sent.
    Connecting,
    /// Writing records, then `PrmEnd`.
    Parameterising,
    /// Waiting for the device's "application ready".
    WaitingReady,
    /// Cyclic data flows.
    Running,
    /// A startup failed; waiting to try again.
    Backoff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Connect,
    Write(usize),
    PrmEnd,
}

#[derive(Debug)]
struct Call {
    step: Step,
    datagram: Vec<u8>,
    sequence: u32,
    sent_at: u64,
    tries: u8,
}

#[derive(Debug)]
enum Phase {
    Identify {
        next: u64,
        xid: u32,
    },
    SetIp {
        frame: Vec<u8>,
        xid: u32,
        sent_at: u64,
        tries: u8,
    },
    Call(Call),
    WaitReady {
        deadline: u64,
    },
    Run,
    Backoff {
        until: u64,
    },
}

#[derive(Debug)]
struct PendingAck {
    frame: Vec<u8>,
    send_seq: u16,
    sent_at: u64,
    tries: u16,
}

/// The AR's state once the station is known.
#[derive(Debug)]
struct Ar {
    mac: Mac,
    ip: Ip,
    /// Where requests go: [`rpc::PORT`] first, then wherever the device
    /// answered from.
    port: u16,
    object: Uuid,
    uuid: Uuid,
    activity: Uuid,
    sequence: u32,
    server_boot: u32,
    /// Whether the device accepted the Connect, and so holds an AR that
    /// a failure must release.
    established: bool,
    /// Known once the Connect is answered; cyclic sending starts then.
    output_frame_id: Option<u16>,
    device_alarm_ref: u16,
    next_send: u64,
    last_cycle: u16,
    /// When the last valid input frame arrived.
    last_input: Option<u64>,
    /// When the startup completed.
    run_at: Option<u64>,
    ready: bool,
    /// Alarm transport, receiving: the next sequence number expected and
    /// the last one taken.
    alarm_expect: u16,
    alarm_taken: u16,
    /// Alarm transport, sending.
    alarm_send: u16,
    pending_ack: Option<PendingAck>,
}

/// A PRNG for UUIDs and transaction ids: unpredictability is not needed,
/// distinctness across restarts is (xorshift64).
#[derive(Debug)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn uuid(&mut self) -> Uuid {
        let mut b = [0; 16];
        b[..8].copy_from_slice(&self.next().to_be_bytes());
        b[8..].copy_from_slice(&self.next().to_be_bytes());
        // Version 4, variant 1, so it reads as a random UUID.
        b[6] = (b[6] & 0x0F) | 0x40;
        b[8] = (b[8] & 0x3F) | 0x80;
        Uuid(b)
    }

    fn xid(&mut self) -> u32 {
        // The high half of the state.
        (self.next() >> 32) as u32
    }
}

/// An IO controller for one device. See the module documentation.
#[derive(Debug)]
pub struct Controller {
    local: Local,
    cfg: DeviceConfig,
    layout: Layout,
    /// Every record to write, in module order.
    records: Vec<(u32, u16, u16, u16, Vec<u8>)>,
    rng: Rng,
    session_key: u16,
    phase: Phase,
    ar: Option<Ar>,
    station: Option<Station>,
    error: Option<String>,
    inputs: Vec<u8>,
    /// Per place: whether the device's IOPS for its input data is good.
    input_good: Vec<bool>,
    outputs: Vec<u8>,
    alarms: Vec<Notification>,
    diff: Vec<ModuleDiff>,
    starts: u32,
}

/// The most alarms kept for the owner to take.
const ALARM_QUEUE: usize = 16;

impl Controller {
    /// A controller for `cfg`, idle until the first `poll`.
    ///
    /// # Errors
    /// A configuration that cannot go on the wire: a bad station name,
    /// no modules, timing out of range, cyclic data over 1440 bytes, an
    /// input frame id outside RT class 1, or a Connect too big for one
    /// datagram.
    pub fn new(local: Local, cfg: DeviceConfig, seed: u64) -> Result<Self, String> {
        if !dcp::valid_station_name(&cfg.station_name) {
            return Err(format!("{:?} is not a station name", cfg.station_name));
        }
        if !dcp::valid_station_name(&local.station_name) {
            return Err(format!("{:?} is not a station name", local.station_name));
        }
        if cfg.modules.is_empty() || cfg.modules.iter().any(|m| m.submodules.is_empty()) {
            return Err("a device needs at least one module, and every module a submodule".into());
        }
        // The clock factors a device takes (p-net `pf_cmdev_check_iocr_param`).
        if !matches!(cfg.send_clock_factor, 1 | 2 | 4 | 8 | 16 | 32 | 64 | 128) {
            return Err(format!(
                "send clock factor {} is not a power of two up to 128",
                cfg.send_clock_factor
            ));
        }
        if !(1..=512).contains(&cfg.reduction_ratio) || !cfg.reduction_ratio.is_power_of_two() {
            return Err(format!(
                "reduction ratio {} is not a power of two up to 512",
                cfg.reduction_ratio
            ));
        }
        let hold_us =
            u64::from(cfg.data_hold_factor) * rt::interval_us(cfg.send_clock_factor, cfg.reduction_ratio);
        if cfg.data_hold_factor == 0 || hold_us > 1_920_000 {
            return Err(format!(
                "data hold factor {} gives a hold time outside 1 cycle to 1.92 s",
                cfg.data_hold_factor
            ));
        }
        if !(eth::FRAME_ID_RTC1_FIRST..=eth::FRAME_ID_RTC1_LAST).contains(&cfg.input_frame_id) {
            return Err(format!(
                "input frame id {:#06x} is outside 0xC000..=0xF7FF",
                cfg.input_frame_id
            ));
        }
        let layout = Layout::new(&cfg.modules).map_err(|e| format!("{e}"))?;
        let mut records = Vec::new();
        for m in &cfg.modules {
            for s in &m.submodules {
                for r in &s.records {
                    if 64 + r.data.len() > rpc::MAX_BODY - 20 {
                        return Err(format!(
                            "record {:#06x} of slot {} is too long for one datagram",
                            r.index, m.slot
                        ));
                    }
                    records.push((m.api, m.slot, s.subslot, r.index, r.data.clone()));
                }
            }
        }
        let c = Self {
            inputs: alloc::vec![0; layout.input_image_len],
            input_good: alloc::vec![false; layout.places.len()],
            outputs: alloc::vec![0; layout.output_image_len],
            local,
            cfg,
            layout,
            records,
            // xorshift has one bad seed.
            rng: Rng(seed | 1),
            session_key: 0,
            phase: Phase::Identify { next: 0, xid: 0 },
            ar: None,
            station: None,
            error: None,
            alarms: Vec::new(),
            diff: Vec::new(),
            starts: 0,
        };
        // Build a Connect now so one that needs fragmenting is a
        // configuration error, not a startup that never succeeds.
        let probe = c.connect_blocks(Uuid::NIL).map_err(|e| format!("{e}"))?;
        if probe.len() + 20 > rpc::MAX_BODY {
            return Err(format!(
                "the Connect for this configuration is {} bytes, more than one datagram carries",
                probe.len()
            ));
        }
        Ok(c)
    }

    #[must_use]
    pub const fn state(&self) -> State {
        match &self.phase {
            Phase::Identify { .. } => State::Identifying,
            Phase::SetIp { .. } => State::Addressing,
            Phase::Call(Call {
                step: Step::Connect, ..
            }) => State::Connecting,
            Phase::Call(_) => State::Parameterising,
            Phase::WaitReady { .. } => State::WaitingReady,
            Phase::Run => State::Running,
            Phase::Backoff { .. } => State::Backoff,
        }
    }

    /// Why the last startup failed or the AR ended; cleared when cyclic
    /// data flows again.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The station as its Identify response described it, once found.
    #[must_use]
    pub const fn station(&self) -> Option<&Station> {
        self.station.as_ref()
    }

    /// The station's MAC address, once found.
    #[must_use]
    pub fn station_mac(&self) -> Option<Mac> {
        self.ar.as_ref().map(|a| a.mac)
    }

    /// How many startups have begun (the first included).
    #[must_use]
    pub const fn starts(&self) -> u32 {
        self.starts
    }

    #[must_use]
    pub const fn layout(&self) -> &Layout {
        &self.layout
    }

    /// The input image, while the AR runs and input frames arrive:
    /// every submodule's input data in configuration order.
    #[must_use]
    pub fn inputs(&self) -> Option<&[u8]> {
        let ar = self.ar.as_ref()?;
        (matches!(self.phase, Phase::Run) && ar.last_input.is_some()).then_some(&self.inputs[..])
    }

    /// Whether the device marks the input data of place `index`
    /// ([`Layout::places`]) as good.
    #[must_use]
    pub fn input_good(&self, index: usize) -> bool {
        self.inputs().is_some() && self.input_good.get(index).copied().unwrap_or(false)
    }

    /// The output image: every submodule's output data in configuration
    /// order. Sent with the next cyclic frame; kept across a restart.
    pub fn outputs_mut(&mut self) -> &mut [u8] {
        &mut self.outputs
    }

    #[must_use]
    pub fn outputs(&self) -> &[u8] {
        &self.outputs
    }

    /// Alarms the device raised since the last call (each already
    /// acknowledged), oldest first.
    pub fn take_alarms(&mut self) -> Vec<Notification> {
        core::mem::take(&mut self.alarms)
    }

    /// What the device said differs from the expected modules but did
    /// not stop the startup (a substitute, a submodule another controller
    /// holds).
    #[must_use]
    pub fn differences(&self) -> &[ModuleDiff] {
        &self.diff
    }

    /// When `poll` next has something to do.
    #[must_use]
    pub fn next_wake(&self, now: u64) -> u64 {
        let phase = match &self.phase {
            Phase::Identify { next, .. } => *next,
            Phase::SetIp { sent_at, .. } => sent_at + REQUEST_TIMEOUT_US,
            Phase::Call(c) => c.sent_at + REQUEST_TIMEOUT_US,
            Phase::WaitReady { deadline } => *deadline,
            Phase::Backoff { until } => *until,
            Phase::Run => now + self.hold_us(),
        };
        let cyclic = self
            .ar
            .as_ref()
            .filter(|a| a.output_frame_id.is_some())
            .map_or(u64::MAX, |a| a.next_send);
        phase.min(cyclic)
    }

    fn interval_us(&self) -> u64 {
        rt::interval_us(self.cfg.send_clock_factor, self.cfg.reduction_ratio)
    }

    fn hold_us(&self) -> u64 {
        self.interval_us() * u64::from(self.cfg.data_hold_factor)
    }

    /// Ends the AR for `why` and schedules another startup.
    fn fail(&mut self, now: u64, why: String, out: &mut Vec<Out>) {
        self.release(out);
        self.error = Some(why);
        self.phase = Phase::Backoff {
            until: now + BACKOFF_US,
        };
    }

    /// Tells the device the AR is over, when there is one, and forgets
    /// it. The response is not waited for.
    pub fn release(&mut self, out: &mut Vec<Out>) {
        if let Some(mut ar) = self.ar.take()
            && ar.established
        {
            let blocks = Control {
                block_type: block::RELEASE_REQ,
                ar_uuid: ar.uuid,
                session_key: self.session_key,
                command: block::COMMAND_RELEASE,
                diff: Vec::new(),
            }
            .encode();
            if let Ok(data) = request(&mut ar, rpc::OP_RELEASE, &blocks) {
                out.push(Out::Udp {
                    ip: ar.ip,
                    port: ar.port,
                    data,
                });
            }
        }
        self.input_good.fill(false);
    }

    /// Drives timers: retries, time-outs, the watchdog and the cyclic
    /// output frame.
    pub fn poll(&mut self, now: u64, out: &mut Vec<Out>) {
        match &mut self.phase {
            Phase::Identify { next, xid } => {
                if now >= *next {
                    if self.starts > 0 && self.error.is_none() && *xid != 0 {
                        self.error = Some(format!("no station named {:?} answers", self.cfg.station_name));
                    }
                    if *xid == 0 {
                        self.starts += 1;
                    }
                    *xid = self.rng.xid().max(1);
                    *next = now + IDENTIFY_PERIOD_US;
                    let payload = dcp::identify_request(*xid, Some(&self.cfg.station_name), 1);
                    out.push(Out::Frame(eth::encode(
                        dcp::MULTICAST_IDENTIFY,
                        self.local.mac,
                        None,
                        eth::FRAME_ID_DCP_IDENTIFY_REQ,
                        &payload,
                    )));
                }
            }
            Phase::SetIp {
                frame,
                sent_at,
                tries,
                ..
            } => {
                if now >= *sent_at + REQUEST_TIMEOUT_US {
                    if *tries >= REQUEST_TRIES {
                        let why = format!(
                            "{:?} did not answer the IP address it was given",
                            self.cfg.station_name
                        );
                        self.fail(now, why, out);
                    } else {
                        *tries += 1;
                        *sent_at = now;
                        out.push(Out::Frame(frame.clone()));
                    }
                }
            }
            Phase::Call(call) => {
                if now >= call.sent_at + REQUEST_TIMEOUT_US {
                    if call.tries >= REQUEST_TRIES {
                        let why = format!(
                            "{:?} did not answer {}",
                            self.cfg.station_name,
                            step_name(call.step)
                        );
                        self.fail(now, why, out);
                    } else if let Some(ar) = &self.ar {
                        call.tries += 1;
                        call.sent_at = now;
                        out.push(Out::Udp {
                            ip: ar.ip,
                            port: ar.port,
                            data: call.datagram.clone(),
                        });
                    }
                }
            }
            Phase::WaitReady { deadline } => {
                if now >= *deadline {
                    let why = format!("{:?} never reported application ready", self.cfg.station_name);
                    self.fail(now, why, out);
                }
            }
            Phase::Run => {}
            Phase::Backoff { until } => {
                if now >= *until {
                    self.phase = Phase::Identify { next: now, xid: 0 };
                    self.poll(now, out);
                    return;
                }
            }
        }
        self.watchdog(now, out);
        self.cyclic(now, out);
        self.resend_alarm_ack(now, out);
    }

    /// Ends the AR when valid input frames have stopped.
    fn watchdog(&mut self, now: u64, out: &mut Vec<Out>) {
        let hold = self.hold_us();
        let Some(ar) = &self.ar else { return };
        let Some(last) = ar.last_input else {
            // Started, and not one frame: give it a second more than the
            // hold time before calling it dead.
            if ar.run_at.is_some_and(|t| now > t + hold + 1_000_000) {
                let why = format!(
                    "{:?} completed the startup but sends no cyclic data",
                    self.cfg.station_name
                );
                self.fail(now, why, out);
            }
            return;
        };
        if now > last + hold {
            let why = format!(
                "{:?} stopped sending cyclic data ({} ms without a valid frame)",
                self.cfg.station_name,
                (now - last) / 1000
            );
            self.fail(now, why, out);
        }
    }

    /// Sends the output frame when it is due.
    fn cyclic(&mut self, now: u64, out: &mut Vec<Out>) {
        let interval = self.interval_us();
        let Some(ar) = &mut self.ar else { return };
        let Some(frame_id) = ar.output_frame_id else {
            return;
        };
        if now < ar.next_send {
            return;
        }
        // A late poll sends one frame and carries on from now: cyclic
        // data wants the newest, not a burst of the missed ones.
        ar.next_send = if now >= ar.next_send + interval {
            now + interval
        } else {
            ar.next_send + interval
        };
        let mut data = alloc::vec![0u8; usize::from(self.layout.output_frame_len)];
        for p in &self.layout.places {
            if let Some(at) = p.output_frame {
                let (at, len) = (usize::from(at), usize::from(p.output_len));
                data[at..at + len].copy_from_slice(&self.outputs[p.output_image..p.output_image + len]);
                data[at + len] = rt::IOXS_GOOD;
            }
            if let Some(at) = p.output_iocs {
                data[usize::from(at)] = rt::IOXS_GOOD;
            }
        }
        // A consumer drops a frame whose counter is not ahead of the last
        // (p-net `pf_cpm_check_cycle`), so it must move even if the clock
        // has not crossed an interval.
        let mut cycle = rt::cycle_counter(now, self.cfg.send_clock_factor, self.cfg.reduction_ratio);
        if !rt::newer(ar.last_cycle, cycle) {
            cycle = ar
                .last_cycle
                .wrapping_add(self.cfg.send_clock_factor.wrapping_mul(self.cfg.reduction_ratio));
        }
        ar.last_cycle = cycle;
        let payload = rt::encode(&data, cycle, rt::STATUS_OK);
        let vlan = self.cfg.vlan.then_some(Vlan::RT);
        out.push(Out::Frame(eth::encode(
            ar.mac,
            self.local.mac,
            vlan,
            frame_id,
            &payload,
        )));
    }

    fn resend_alarm_ack(&mut self, now: u64, out: &mut Vec<Out>) {
        let Some(ar) = &mut self.ar else { return };
        let Some(p) = &mut ar.pending_ack else { return };
        if now < p.sent_at + RTA_TIMEOUT_US * u64::from(RTA_TIMEOUT_FACTOR) {
            return;
        }
        if p.tries >= RTA_RETRIES {
            ar.pending_ack = None;
        } else {
            p.tries += 1;
            p.sent_at = now;
            out.push(Out::Frame(p.frame.clone()));
        }
    }

    /// A raw frame arrived. Frames that are not PROFINET, not for this
    /// controller or not from its device are ignored.
    pub fn on_frame(&mut self, now: u64, frame: &[u8], out: &mut Vec<Out>) {
        let Ok(f) = eth::parse(frame) else { return };
        if f.dst != self.local.mac {
            return;
        }
        match f.frame_id {
            eth::FRAME_ID_DCP_IDENTIFY_RES => self.on_identify(now, f.src, f.payload, out),
            eth::FRAME_ID_DCP_GET_SET => self.on_set_response(now, f.src, f.payload, out),
            eth::FRAME_ID_ALARM_HIGH | eth::FRAME_ID_ALARM_LOW => self.on_alarm(now, &f, out),
            id if id == self.cfg.input_frame_id => self.on_cyclic(now, f.src, f.payload),
            _ => {}
        }
    }

    fn on_identify(&mut self, now: u64, src: Mac, payload: &[u8], out: &mut Vec<Out>) {
        let Phase::Identify { xid, .. } = self.phase else {
            return;
        };
        let Ok((got, station)) = dcp::parse_identify_response(payload) else {
            return;
        };
        if got != xid || station.name != self.cfg.station_name {
            return;
        }
        let ids = (station.vendor_id, station.device_id);
        if self.cfg.vendor_id.is_some_and(|v| v != ids.0) || self.cfg.device_id.is_some_and(|d| d != ids.1) {
            let why = format!(
                "{:?} is vendor {:#06x} device {:#06x}, not the {:#06x} / {:#06x} expected",
                station.name,
                ids.0,
                ids.1,
                self.cfg.vendor_id.unwrap_or(ids.0),
                self.cfg.device_id.unwrap_or(ids.1)
            );
            self.station = Some(station);
            self.fail(now, why, out);
            return;
        }
        let instance = station.instance.unwrap_or(self.cfg.instance);
        self.ar = Some(Ar {
            mac: src,
            ip: station.ip,
            port: rpc::PORT,
            object: rpc::object_uuid(instance, ids.1, ids.0),
            uuid: Uuid::NIL,
            activity: Uuid::NIL,
            sequence: 0,
            server_boot: 0,
            established: false,
            output_frame_id: None,
            device_alarm_ref: 0,
            next_send: 0,
            last_cycle: 0,
            last_input: None,
            run_at: None,
            ready: false,
            alarm_expect: alarm::SEQ_FIRST,
            alarm_taken: alarm::SEQ_NONE,
            alarm_send: alarm::SEQ_FIRST,
            pending_ack: None,
        });
        let have = station.ip;
        self.station = Some(station);
        match self.cfg.address {
            Some(a) if a.ip != have => {
                let xid = self.rng.xid().max(1);
                let set = dcp::Set::Ip {
                    ip: a.ip,
                    mask: a.mask,
                    gateway: a.gateway,
                };
                // An IP block always fits.
                let payload = dcp::set_request(xid, &set, false).unwrap_or_default();
                let frame = eth::encode(src, self.local.mac, None, eth::FRAME_ID_DCP_GET_SET, &payload);
                out.push(Out::Frame(frame.clone()));
                self.phase = Phase::SetIp {
                    frame,
                    xid,
                    sent_at: now,
                    tries: 1,
                };
            }
            None if have == [0; 4] => {
                let why = format!(
                    "{:?} has no IP address and the device line gives it none",
                    self.cfg.station_name
                );
                self.fail(now, why, out);
            }
            _ => self.connect(now, out),
        }
    }

    fn on_set_response(&mut self, now: u64, src: Mac, payload: &[u8], out: &mut Vec<Out>) {
        let Phase::SetIp { xid, .. } = self.phase else {
            return;
        };
        let Ok((got, error)) = dcp::parse_set_response(payload) else {
            return;
        };
        if got != xid || self.ar.as_ref().is_none_or(|a| a.mac != src) {
            return;
        }
        if error != dcp::ERROR_NONE {
            let why = format!(
                "{:?} refused the IP address (DCP block error {error})",
                self.cfg.station_name
            );
            self.fail(now, why, out);
            return;
        }
        if let (Some(ar), Some(a)) = (&mut self.ar, self.cfg.address) {
            ar.ip = a.ip;
            if let Some(s) = &mut self.station {
                (s.ip, s.mask, s.gateway) = (a.ip, a.mask, a.gateway);
            }
        }
        self.connect(now, out);
    }

    fn connect_blocks(&self, ar_uuid: Uuid) -> Result<Vec<u8>, DecodeError> {
        let cfg = &self.cfg;
        let iocr = |iocr_type, reference, data_length, frame_id, apis| IocrReq {
            iocr_type,
            reference,
            properties: block::RT_CLASS_1,
            data_length,
            frame_id,
            send_clock_factor: cfg.send_clock_factor,
            reduction_ratio: cfg.reduction_ratio,
            phase: 1,
            // "Best effort" (p-net `pf_cmdev_check_iocr_param`).
            frame_send_offset: 0xFFFF_FFFF,
            watchdog_factor: cfg.data_hold_factor,
            data_hold_factor: cfg.data_hold_factor,
            tag: Vlan::RT.tci(),
            multicast_mac: [0; 6],
            apis,
        };
        let mut properties = block::AR_PROPERTIES_ACTIVE;
        if cfg.advanced_startup {
            properties |= block::AR_PROPERTIES_STARTUP_ADVANCED;
        }
        ConnectReq {
            ar: ArReq {
                ar_type: block::AR_TYPE_SINGLE,
                ar_uuid,
                session_key: self.session_key,
                initiator_mac: self.local.mac,
                initiator_object: rpc::object_uuid(1, self.local.device_id, self.local.vendor_id),
                properties,
                // 20 s for each step of the startup.
                activity_timeout: 200,
                udp_rt_port: eth::ETHERTYPE_PROFINET,
                station_name: self.local.station_name.clone(),
            },
            iocrs: alloc::vec![
                iocr(
                    block::IOCR_INPUT,
                    1,
                    self.layout.input_frame_len,
                    cfg.input_frame_id,
                    self.layout.input_apis()
                ),
                iocr(
                    block::IOCR_OUTPUT,
                    2,
                    self.layout.output_frame_len,
                    block::FRAME_ID_ANY,
                    self.layout.output_apis()
                ),
            ],
            alarm: AlarmCrReq {
                properties: 0,
                rta_timeout_factor: RTA_TIMEOUT_FACTOR,
                rta_retries: RTA_RETRIES,
                local_alarm_ref: ALARM_REF,
                max_alarm_data: 200,
                tag_high: Vlan { priority: 6, id: 0 }.tci(),
                tag_low: Vlan { priority: 5, id: 0 }.tci(),
            },
            expected: rt::expected(&cfg.modules),
        }
        .encode()
    }

    fn connect(&mut self, now: u64, out: &mut Vec<Out>) {
        self.session_key = self.session_key.wrapping_add(1).max(1);
        let (uuid, activity) = (self.rng.uuid(), self.rng.uuid());
        if let Some(ar) = &mut self.ar {
            ar.uuid = uuid;
            ar.activity = activity;
            ar.sequence = 0;
            ar.server_boot = 0;
            ar.port = rpc::PORT;
        }
        self.diff.clear();
        match self.connect_blocks(uuid) {
            Ok(blocks) => self.call(now, Step::Connect, rpc::OP_CONNECT, &blocks, out),
            Err(e) => self.fail(now, format!("{e}"), out),
        }
    }

    /// Sends one RPC and waits for its answer.
    fn call(&mut self, now: u64, step: Step, opnum: u16, blocks: &[u8], out: &mut Vec<Out>) {
        let Some(ar) = &mut self.ar else { return };
        let sequence = ar.sequence;
        match request(ar, opnum, blocks) {
            Ok(datagram) => {
                out.push(Out::Udp {
                    ip: ar.ip,
                    port: ar.port,
                    data: datagram.clone(),
                });
                self.phase = Phase::Call(Call {
                    step,
                    datagram,
                    sequence,
                    sent_at: now,
                    tries: 1,
                });
            }
            Err(e) => self.fail(now, format!("{}: {e}", step_name(step)), out),
        }
    }

    /// The step after `step` succeeded.
    fn advance(&mut self, now: u64, step: Step, out: &mut Vec<Out>) {
        let next = match step {
            Step::Connect => 0,
            Step::Write(i) => i + 1,
            Step::PrmEnd => {
                self.phase = if let Some(ar) = self.ar.as_mut().filter(|a| a.ready) {
                    ar.run_at = Some(now);
                    Phase::Run
                } else {
                    Phase::WaitReady {
                        deadline: now + READY_TIMEOUT_US,
                    }
                };
                return;
            }
        };
        let Some(ar) = &self.ar else { return };
        if let Some((api, slot, subslot, index, data)) = self.records.get(next) {
            let blocks = WriteReq {
                // Truncation is fine: the field only pairs a response
                // with its request.
                sequence: u16::try_from(next & 0xFFFF).unwrap_or(0),
                ar_uuid: ar.uuid,
                api: *api,
                slot: *slot,
                subslot: *subslot,
                index: *index,
                data: data.clone(),
            }
            .encode();
            self.call(now, Step::Write(next), rpc::OP_WRITE, &blocks, out);
        } else {
            let blocks = Control {
                block_type: block::PRM_END_REQ,
                ar_uuid: ar.uuid,
                session_key: self.session_key,
                command: block::COMMAND_PRM_END,
                diff: Vec::new(),
            }
            .encode();
            self.call(now, Step::PrmEnd, rpc::OP_CONTROL, &blocks, out);
        }
    }

    /// A datagram arrived at the controller's [`rpc::PORT`] socket.
    pub fn on_udp(&mut self, now: u64, ip: Ip, port: u16, data: &[u8], out: &mut Vec<Out>) {
        let Some(ar) = &self.ar else { return };
        if ip != ar.ip {
            return;
        }
        let Ok((header, body)) = Header::decode(data) else {
            return;
        };
        if header.packet_type == rpc::PT_REQUEST {
            self.on_request(now, ip, port, &header, body, out);
            return;
        }
        let Phase::Call(call) = &mut self.phase else {
            return;
        };
        if header.activity != ar.activity || header.sequence != call.sequence {
            return;
        }
        let step = call.step;
        match header.packet_type {
            rpc::PT_RESPONSE => {}
            rpc::PT_WORKING => {
                // The device is still on it: wait a full time-out more.
                call.sent_at = now;
                return;
            }
            rpc::PT_FAULT | rpc::PT_REJECT => {
                let code = body.get(..4).map_or(0, |b| match header.order {
                    Order::Big => u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
                    Order::Little => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                });
                let why = format!(
                    "{:?} rejected {} (RPC status {code:#010x})",
                    self.cfg.station_name,
                    step_name(step)
                );
                self.fail(now, why, out);
                return;
            }
            _ => return,
        }
        if header.fragmented() {
            let why = format!(
                "{:?} answered {} in fragments, which this controller does not reassemble",
                self.cfg.station_name,
                step_name(step)
            );
            self.fail(now, why, out);
            return;
        }
        let (status, blocks) = match rpc::parse_response_body(header.order, body) {
            Ok(v) => v,
            Err(e) => {
                let why = format!("{}: {e}", step_name(step));
                self.fail(now, why, out);
                return;
            }
        };
        if let Some(ar) = &mut self.ar {
            ar.server_boot = header.server_boot;
            ar.port = port;
            ar.established |= status.is_ok();
        }
        if !status.is_ok() {
            let why = format!(
                "{:?} refused {}: {status}",
                self.cfg.station_name,
                step_name(step)
            );
            self.fail(now, why, out);
            return;
        }
        if let Err(why) = self.on_response(now, step, blocks) {
            self.fail(now, why, out);
            return;
        }
        self.advance(now, step, out);
    }

    fn on_response(&mut self, now: u64, step: Step, blocks: &[u8]) -> Result<(), String> {
        let name = &self.cfg.station_name;
        let Some(ar) = &mut self.ar else {
            return Ok(());
        };
        match step {
            Step::Connect => {
                let res = ConnectRes::decode(blocks).map_err(|e| format!("Connect response: {e}"))?;
                if res.ar.ar_uuid != ar.uuid {
                    return Err(format!("{name:?} answered the Connect for another AR"));
                }
                let output = res
                    .iocrs
                    .iter()
                    .find(|c| c.iocr_type == block::IOCR_OUTPUT)
                    .ok_or_else(|| format!("{name:?} named no output frame id"))?;
                if let Some(input) = res.iocrs.iter().find(|c| c.iocr_type == block::IOCR_INPUT)
                    && input.frame_id != self.cfg.input_frame_id
                {
                    return Err(format!(
                        "{name:?} will send frame id {:#06x}, not the {:#06x} asked for",
                        input.frame_id, self.cfg.input_frame_id
                    ));
                }
                let wrong = describe_diff(&res.diff);
                if !wrong.is_empty() {
                    return Err(format!("{name:?} is not built as configured: {wrong}"));
                }
                self.diff = res.diff;
                ar.mac = res.ar.responder_mac;
                ar.device_alarm_ref = res.alarm.local_alarm_ref;
                ar.output_frame_id = Some(output.frame_id);
                ar.next_send = now;
                Ok(())
            }
            Step::Write(i) => {
                let res = WriteRes::decode(blocks).map_err(|e| format!("write response: {e}"))?;
                if res.status.is_ok() {
                    Ok(())
                } else {
                    let index = self.records.get(i).map_or(0, |r| r.3);
                    Err(format!("{name:?} refused record {index:#06x}: {}", res.status))
                }
            }
            Step::PrmEnd => {
                let res = Control::decode(blocks).map_err(|e| format!("PrmEnd response: {e}"))?;
                if res.block_type == block::PRM_END_RES && res.command == block::COMMAND_DONE {
                    Ok(())
                } else {
                    Err(format!("{name:?} answered PrmEnd with something else"))
                }
            }
        }
    }

    /// The device's call to the controller: "application ready".
    fn on_request(&mut self, now: u64, ip: Ip, port: u16, header: &Header, body: &[u8], out: &mut Vec<Out>) {
        if header.interface != rpc::IF_CONTROLLER || header.opnum != rpc::OP_CONTROL || header.fragmented() {
            return;
        }
        let Ok((args_max, blocks)) = rpc::parse_request_body(header.order, body) else {
            return;
        };
        let Ok(req) = Control::decode(blocks) else { return };
        let Some(ar) = &mut self.ar else { return };
        if req.block_type != block::APPLICATION_READY_REQ || req.ar_uuid != ar.uuid {
            return;
        }
        let wrong = describe_diff(&req.diff);
        if !wrong.is_empty() {
            let why = format!("{:?} is not built as configured: {wrong}", self.cfg.station_name);
            self.fail(now, why, out);
            return;
        }
        let answer = Control {
            block_type: block::APPLICATION_READY_RES,
            ar_uuid: ar.uuid,
            session_key: req.session_key,
            command: block::COMMAND_DONE,
            diff: Vec::new(),
        }
        .encode();
        let response = Header {
            packet_type: rpc::PT_RESPONSE,
            // What p-net's own responses carry (`pf_cmrpc_dce_packet`).
            flags: rpc::FLAG_NO_FACK | rpc::FLAG_IDEMPOTENT,
            // A boot time must not be 0; the session key's origin will do.
            server_boot: 1,
            ..*header
        };
        if let Ok(data) = response.encode(&rpc::response_body(header.order, Status::OK, args_max, &answer)) {
            out.push(Out::Udp { ip, port, data });
        }
        ar.ready = true;
        if !req.diff.is_empty() {
            self.diff = req.diff;
        }
        if matches!(self.phase, Phase::WaitReady { .. }) {
            ar.run_at = Some(now);
            self.phase = Phase::Run;
        }
    }

    fn on_cyclic(&mut self, now: u64, src: Mac, payload: &[u8]) {
        let Some(ar) = &mut self.ar else { return };
        if src != ar.mac || ar.output_frame_id.is_none() {
            return;
        }
        let Ok(c) = rt::parse(payload, self.layout.input_frame_len) else {
            return;
        };
        if c.data_status & rt::STATUS_DATA_VALID == 0 {
            return;
        }
        for (i, p) in self.layout.places.iter().enumerate() {
            let Some(at) = p.input_frame else { continue };
            let (at, len) = (usize::from(at), usize::from(p.input_len));
            let good = c.data[at + len] & rt::IOXS_GOOD != 0;
            self.input_good[i] = good;
            if good {
                self.inputs[p.input_image..p.input_image + len].copy_from_slice(&c.data[at..at + len]);
            }
        }
        ar.last_input = Some(now);
        if matches!(self.phase, Phase::Run) {
            self.error = None;
        }
    }

    fn on_alarm(&mut self, now: u64, f: &eth::Frame<'_>, out: &mut Vec<Out>) {
        let Some(ar) = &mut self.ar else { return };
        if f.src != ar.mac {
            return;
        }
        let Ok(rta) = Rta::parse(f.payload) else { return };
        if rta.dst_ref != ALARM_REF {
            return;
        }
        let high = f.frame_id == eth::FRAME_ID_ALARM_HIGH;
        let vlan = Vlan {
            priority: if high { 6 } else { 5 },
            id: 0,
        };
        let mac = self.local.mac;
        let reply = |pdu: &Rta| eth::encode(f.src, mac, Some(vlan), f.frame_id, &pdu.encode());
        match rta.pdu_type {
            alarm::PDU_DATA => {
                let fresh = rta.send_seq == ar.alarm_expect;
                if !fresh && rta.send_seq != ar.alarm_taken {
                    return;
                }
                if fresh {
                    ar.alarm_taken = ar.alarm_expect;
                    ar.alarm_expect = alarm::next_seq(ar.alarm_expect);
                }
                // The transport acknowledgement, for a repeat too.
                out.push(Out::Frame(reply(&Rta {
                    dst_ref: ar.device_alarm_ref,
                    src_ref: ALARM_REF,
                    pdu_type: alarm::PDU_ACK,
                    tack: false,
                    send_seq: alarm::SEQ_NONE,
                    ack_seq: ar.alarm_taken,
                    var: Vec::new(),
                })));
                if !fresh {
                    return;
                }
                let Ok(n) = Notification::parse(&rta.var) else {
                    return;
                };
                // Then the alarm's own acknowledgement, which the device
                // acknowledges at the transport in turn.
                let send_seq = ar.alarm_send;
                let frame = reply(&Rta {
                    dst_ref: ar.device_alarm_ref,
                    src_ref: ALARM_REF,
                    pdu_type: alarm::PDU_DATA,
                    tack: true,
                    send_seq,
                    ack_seq: ar.alarm_taken,
                    var: n.ack(Status::OK),
                });
                out.push(Out::Frame(frame.clone()));
                ar.pending_ack = Some(PendingAck {
                    frame,
                    send_seq,
                    sent_at: now,
                    tries: 1,
                });
                if self.alarms.len() == ALARM_QUEUE {
                    self.alarms.remove(0);
                }
                self.alarms.push(n);
            }
            alarm::PDU_ACK => {
                if ar.pending_ack.as_ref().is_some_and(|p| p.send_seq == rta.ack_seq) {
                    ar.pending_ack = None;
                    ar.alarm_send = alarm::next_seq(ar.alarm_send);
                }
            }
            alarm::PDU_ERR => {
                let why = format!(
                    "{:?} ended the connection (alarm transport error)",
                    self.cfg.station_name
                );
                self.fail(now, why, out);
            }
            _ => {}
        }
    }
}

/// One request on the AR's activity, with the next sequence number.
fn request(ar: &mut Ar, opnum: u16, blocks: &[u8]) -> Result<Vec<u8>, DecodeError> {
    let order = Order::Little;
    let header = Header {
        packet_type: rpc::PT_REQUEST,
        flags: rpc::FLAG_IDEMPOTENT,
        order,
        object: ar.object,
        interface: rpc::IF_DEVICE,
        activity: ar.activity,
        server_boot: ar.server_boot,
        sequence: ar.sequence,
        opnum,
        fragment: 0,
    };
    ar.sequence = ar.sequence.wrapping_add(1);
    // The most the response's blocks may be: one datagram's worth.
    let args_max = u32::try_from(rpc::MAX_BODY - 20).unwrap_or(u32::MAX);
    header.encode(&rpc::request_body(order, args_max, blocks))
}

const fn step_name(step: Step) -> &'static str {
    match step {
        Step::Connect => "the Connect",
        Step::Write(_) => "a record write",
        Step::PrmEnd => "PrmEnd",
    }
}

/// The differences that mean the device cannot run as configured, in
/// words; empty when what differs is tolerable.
fn describe_diff(diff: &[ModuleDiff]) -> String {
    let mut out = String::new();
    let mut add = |s: String| {
        if !out.is_empty() {
            out.push_str("; ");
        }
        out.push_str(&s);
    };
    for m in diff {
        match m.state {
            block::MODULE_NONE => add(format!("slot {} is empty", m.slot)),
            block::MODULE_WRONG => add(format!("slot {} holds module {:#x}", m.slot, m.ident)),
            _ => {}
        }
        for s in &m.submodules {
            match s.ident_info() {
                2 => add(format!(
                    "slot {} subslot {} holds submodule {:#x}",
                    m.slot, s.subslot, s.ident
                )),
                3 => add(format!("slot {} subslot {} is empty", m.slot, s.subslot)),
                _ if s.ar_info() >= 2 => add(format!(
                    "slot {} subslot {} is held by another controller",
                    m.slot, s.subslot
                )),
                _ => {}
            }
        }
    }
    out
}
