//! An IO device as a state machine without I/O: what the controller
//! talks to in tests, in helmsman-io's simulator, and on a bench with no
//! real device to hand.
//!
//! It answers DCP (Identify, Set of the IP address and the name), takes
//! one AR (Connect, record writes, `PrmEnd`, Release), reports
//! "application ready", exchanges cyclic frames at the offsets the
//! controller's Connect chose, and raises alarms. Like the controller
//! ([`crate::controller`]) it is handed frames, datagrams and the time,
//! and fills `out`.
//!
//! It is a test device, not a certifiable stack: one AR, no record
//! reads, no LLDP or port submodules of its own, no persistence. What it
//! does on the wire follows p-net, the open device stack the controller
//! was written against (docs/SOURCES.md).

use alloc::string::String;
use alloc::vec::Vec;

use crate::alarm::{self, Ack, Notification, Rta};
use crate::block::{
    self, AlarmCrRes, ArRes, ConnectReq, ConnectRes, Control, IocrRes, ModuleDiff, SubmoduleDiff, WriteReq,
    WriteRes,
};
use crate::codec::Order;
use crate::controller::Out;
use crate::dcp::{self, Ip, Station};
use crate::eth::{self, Mac, Vlan};
use crate::rpc::{self, Header, Status, Uuid};
use crate::rt::{self, Layout, Module};

/// The device's alarm reference.
const ALARM_REF: u16 = 2;
/// The frame id the device picks for the output CR when asked to.
const OUTPUT_FRAME_ID: u16 = 0xC100;
/// How often "application ready" is repeated until it is answered.
const READY_PERIOD_US: u64 = 1_000_000;
/// PNIO status: Connect refused, faulty IOCR block (p-net
/// `PNET_ERROR_CODE_CONNECT` 0xDB, `PNET_ERROR_DECODE_PNIO` 0x81,
/// `PNET_ERROR_CODE_1_CONN_FAULTY_IOCR_BLOCK_REQ` 0x02).
const STATUS_FAULTY_IOCR: Status = Status {
    code: 0xDB,
    decode: 0x81,
    code1: 0x02,
    code2: 0x00,
};
/// PNIO status: a write refused (`PNET_ERROR_CODE_WRITE` 0xDF,
/// `PNET_ERROR_DECODE_PNIORW` 0x80, "invalid index" 0xB0).
const STATUS_INVALID_INDEX: Status = Status {
    code: 0xDF,
    decode: 0x80,
    code1: 0xB0,
    code2: 0x00,
};

/// Things a test can make the device do wrong.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Faults {
    /// Stop sending cyclic frames (a cable pulled, a device hung).
    pub silent: bool,
    /// Refuse writes of this record index.
    pub refuse_record: Option<u16>,
    /// Never report "application ready".
    pub never_ready: bool,
    /// Ignore RPCs altogether.
    pub deaf: bool,
}

/// Where one submodule's bytes are, once an AR is up.
#[derive(Debug, Clone, Copy)]
struct Spot {
    input_len: usize,
    output_len: usize,
    input_image: usize,
    output_image: usize,
    /// Offsets in the input frame: its data (IOPS after it), and the
    /// IOCS for its outputs.
    input_frame: Option<usize>,
    input_iocs: Option<usize>,
    /// Offsets in the output frame.
    output_frame: Option<usize>,
}

#[derive(Debug)]
struct Ar {
    uuid: Uuid,
    session_key: u16,
    controller_mac: Mac,
    controller_ip: Ip,
    controller_alarm_ref: u16,
    input_frame_id: u16,
    output_frame_id: u16,
    input_len: usize,
    output_len: u16,
    interval_us: u64,
    hold_us: u64,
    clock: (u16, u16),
    vlan: Option<Vlan>,
    spots: Vec<Spot>,
    next_send: u64,
    last_cycle: u16,
    last_output: Option<u64>,
    /// "Application ready": when to send it next, and its call.
    ready_at: Option<u64>,
    ready_activity: Uuid,
    ready_done: bool,
    alarm_send: u16,
    alarm_expect: u16,
    alarm_taken: u16,
}

/// A simulated IO device. See the module documentation.
#[derive(Debug)]
pub struct Device {
    mac: Mac,
    station: Station,
    modules: Vec<Module>,
    layout: Layout,
    inputs: Vec<u8>,
    outputs: Vec<u8>,
    ar: Option<Ar>,
    seed: u64,
    /// Records written, newest last: slot, subslot, index, data.
    pub records: Vec<(u16, u16, u16, Vec<u8>)>,
    /// Alarms the controller has acknowledged.
    pub alarms_acked: u32,
    /// Connects accepted.
    pub connects: u32,
    pub faults: Faults,
}

impl Device {
    /// A device with these modules plugged.
    ///
    /// # Panics
    /// If the modules' cyclic data cannot fit a frame: a bug in the
    /// test that built them.
    #[must_use]
    pub fn new(mac: Mac, station: Station, modules: Vec<Module>, seed: u64) -> Self {
        let layout = Layout::new(&modules).expect("simulated device's modules fit a frame");
        Self {
            mac,
            station,
            inputs: alloc::vec![0; layout.input_image_len],
            outputs: alloc::vec![0; layout.output_image_len],
            modules,
            layout,
            ar: None,
            seed: seed | 1,
            records: Vec::new(),
            alarms_acked: 0,
            connects: 0,
            faults: Faults::default(),
        }
    }

    #[must_use]
    pub const fn station(&self) -> &Station {
        &self.station
    }

    #[must_use]
    pub const fn mac(&self) -> Mac {
        self.mac
    }

    /// Whether a controller is connected.
    #[must_use]
    pub const fn connected(&self) -> bool {
        self.ar.is_some()
    }

    /// The input image: what the device reports, in its modules' order.
    pub fn inputs_mut(&mut self) -> &mut [u8] {
        &mut self.inputs
    }

    /// The output image as the controller last drove it; zeros with no
    /// AR or once output frames stop.
    #[must_use]
    pub fn outputs(&self) -> &[u8] {
        &self.outputs
    }

    fn random(&mut self) -> u64 {
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.seed = x;
        x
    }

    fn drop_ar(&mut self) {
        self.ar = None;
        self.outputs.fill(0);
    }

    /// Timers: the cyclic input frame, "application ready", and the
    /// output watchdog.
    pub fn poll(&mut self, now: u64, out: &mut Vec<Out>) {
        let Some(ar) = &mut self.ar else { return };
        if ar.last_output.is_some_and(|t| now > t + ar.hold_us) {
            self.drop_ar();
            return;
        }
        if !ar.ready_done && !self.faults.never_ready && ar.ready_at.is_some_and(|t| now >= t) {
            ar.ready_at = Some(now + READY_PERIOD_US);
            let blocks = Control {
                block_type: block::APPLICATION_READY_REQ,
                ar_uuid: ar.uuid,
                session_key: ar.session_key,
                command: block::COMMAND_APPLICATION_READY,
                diff: Vec::new(),
            }
            .encode();
            let header = Header {
                packet_type: rpc::PT_REQUEST,
                flags: rpc::FLAG_IDEMPOTENT,
                order: Order::Big,
                object: rpc::object_uuid(1, 0, 0),
                interface: rpc::IF_CONTROLLER,
                activity: ar.ready_activity,
                server_boot: 0,
                sequence: 0,
                opnum: rpc::OP_CONTROL,
                fragment: 0,
            };
            if let Ok(data) = header.encode(&rpc::request_body(Order::Big, 1000, &blocks)) {
                out.push(Out::Udp {
                    ip: ar.controller_ip,
                    port: rpc::PORT,
                    data,
                });
            }
        }
        if self.faults.silent || now < ar.next_send {
            return;
        }
        ar.next_send = if now >= ar.next_send + ar.interval_us {
            now + ar.interval_us
        } else {
            ar.next_send + ar.interval_us
        };
        let mut data = alloc::vec![0u8; ar.input_len];
        for s in &ar.spots {
            if let Some(at) = s.input_frame {
                data[at..at + s.input_len]
                    .copy_from_slice(&self.inputs[s.input_image..s.input_image + s.input_len]);
                data[at + s.input_len] = rt::IOXS_GOOD;
            }
            if let Some(at) = s.input_iocs {
                data[at] = rt::IOXS_GOOD;
            }
        }
        let mut cycle = rt::cycle_counter(now, ar.clock.0, ar.clock.1);
        if !rt::newer(ar.last_cycle, cycle) {
            cycle = ar.last_cycle.wrapping_add(ar.clock.0.wrapping_mul(ar.clock.1));
        }
        ar.last_cycle = cycle;
        let payload = rt::encode(&data, cycle, rt::STATUS_OK);
        out.push(Out::Frame(eth::encode(
            ar.controller_mac,
            self.mac,
            ar.vlan,
            ar.input_frame_id,
            &payload,
        )));
    }

    /// A raw frame arrived.
    pub fn on_frame(&mut self, now: u64, frame: &[u8], out: &mut Vec<Out>) {
        let Ok(f) = eth::parse(frame) else { return };
        match f.frame_id {
            eth::FRAME_ID_DCP_IDENTIFY_REQ if f.dst == dcp::MULTICAST_IDENTIFY => {
                if let Ok(dcp::Request::Identify { xid, name, .. }) = dcp::parse_request(f.payload)
                    && name.is_none_or(|n| n == self.station.name)
                {
                    let payload = dcp::identify_response(xid, &self.station);
                    out.push(Out::Frame(eth::encode(
                        f.src,
                        self.mac,
                        None,
                        eth::FRAME_ID_DCP_IDENTIFY_RES,
                        &payload,
                    )));
                }
            }
            eth::FRAME_ID_DCP_GET_SET if f.dst == self.mac => self.on_set(f.src, f.payload, out),
            eth::FRAME_ID_ALARM_HIGH | eth::FRAME_ID_ALARM_LOW if f.dst == self.mac => {
                self.on_alarm(&f, out);
            }
            id if f.dst == self.mac => self.on_cyclic(now, id, f.src, f.payload),
            _ => {}
        }
    }

    fn on_set(&mut self, src: Mac, payload: &[u8], out: &mut Vec<Out>) {
        let answer = match dcp::parse_request(payload) {
            Ok(dcp::Request::Set { xid, set, .. }) => {
                let (option, sub) = dcp::set_option(&set);
                // A real device refuses while an AR runs (block error 6).
                let error = if self.ar.is_some() {
                    dcp::ERROR_IN_OPERATION
                } else {
                    match set {
                        dcp::Set::Ip { ip, mask, gateway } => {
                            (self.station.ip, self.station.mask, self.station.gateway) = (ip, mask, gateway);
                        }
                        dcp::Set::Name(name) => self.station.name = name,
                    }
                    dcp::ERROR_NONE
                };
                dcp::set_response(xid, option, sub, error)
            }
            Ok(dcp::Request::Unsupported { service, xid }) => dcp::unsupported_response(service, xid),
            _ => return,
        };
        out.push(Out::Frame(eth::encode(
            src,
            self.mac,
            None,
            eth::FRAME_ID_DCP_GET_SET,
            &answer,
        )));
    }

    fn on_cyclic(&mut self, now: u64, frame_id: u16, src: Mac, payload: &[u8]) {
        let Some(ar) = &mut self.ar else { return };
        if frame_id != ar.output_frame_id || src != ar.controller_mac {
            return;
        }
        let Ok(c) = rt::parse(payload, ar.output_len) else {
            return;
        };
        if c.data_status & rt::STATUS_DATA_VALID == 0 {
            return;
        }
        for s in &ar.spots {
            let Some(at) = s.output_frame else { continue };
            if c.data[at + s.output_len] & rt::IOXS_GOOD != 0 {
                self.outputs[s.output_image..s.output_image + s.output_len]
                    .copy_from_slice(&c.data[at..at + s.output_len]);
            }
        }
        ar.last_output = Some(now);
    }

    /// A datagram arrived at the device's [`rpc::PORT`] socket.
    pub fn on_udp(&mut self, now: u64, ip: Ip, port: u16, data: &[u8], out: &mut Vec<Out>) {
        if self.faults.deaf {
            return;
        }
        let Ok((header, body)) = Header::decode(data) else {
            return;
        };
        if header.packet_type == rpc::PT_RESPONSE {
            // The controller's answer to "application ready".
            if let Some(ar) = &mut self.ar
                && header.activity == ar.ready_activity
                && let Ok((status, blocks)) = rpc::parse_response_body(header.order, body)
                && status.is_ok()
                && Control::decode(blocks).is_ok_and(|c| c.command == block::COMMAND_DONE)
            {
                ar.ready_done = true;
            }
            return;
        }
        if header.packet_type != rpc::PT_REQUEST || header.interface != rpc::IF_DEVICE || header.fragmented()
        {
            return;
        }
        let Ok((args_max, blocks)) = rpc::parse_request_body(header.order, body) else {
            return;
        };
        let (status, answer) = match header.opnum {
            rpc::OP_CONNECT => self.on_connect(now, ip, blocks),
            rpc::OP_WRITE => self.on_write(blocks),
            rpc::OP_CONTROL => self.on_control(now, blocks),
            rpc::OP_RELEASE => self.on_release(blocks),
            _ => return,
        };
        let response = Header {
            packet_type: rpc::PT_RESPONSE,
            flags: rpc::FLAG_NO_FACK | rpc::FLAG_IDEMPOTENT,
            server_boot: 1,
            ..header
        };
        if let Ok(data) = response.encode(&rpc::response_body(header.order, status, args_max, &answer)) {
            out.push(Out::Udp { ip, port, data });
        }
    }

    /// What differs between the modules plugged and those expected.
    fn diff(&self, expected: &[Module]) -> Vec<ModuleDiff> {
        let mut diff = Vec::new();
        for e in expected {
            let have = self.modules.iter().find(|m| (m.api, m.slot) == (e.api, e.slot));
            let mut d = ModuleDiff {
                api: e.api,
                slot: e.slot,
                ident: have.map_or(0, |m| m.ident),
                state: match have {
                    None => block::MODULE_NONE,
                    Some(m) if m.ident != e.ident => block::MODULE_WRONG,
                    Some(_) => block::MODULE_PROPER,
                },
                submodules: Vec::new(),
            };
            for es in &e.submodules {
                let sub = have.and_then(|m| m.submodules.iter().find(|s| s.subslot == es.subslot));
                match sub {
                    Some(s) if (s.ident, s.input, s.output) == (es.ident, es.input, es.output) => {}
                    Some(s) => d.submodules.push(SubmoduleDiff {
                        subslot: es.subslot,
                        ident: s.ident,
                        state: SubmoduleDiff::WRONG,
                    }),
                    None => d.submodules.push(SubmoduleDiff {
                        subslot: es.subslot,
                        ident: 0,
                        state: SubmoduleDiff::NO_SUBMODULE,
                    }),
                }
            }
            if d.state != block::MODULE_PROPER || !d.submodules.is_empty() {
                diff.push(d);
            }
        }
        diff
    }

    /// Where the controller put each submodule this device really has
    /// and the controller expects; `None` when one of them has no place
    /// in a frame.
    fn spots(
        &self,
        expected: &[Module],
        diff: &[ModuleDiff],
        input: &block::IocrReq,
        output: &block::IocrReq,
    ) -> Option<Vec<Spot>> {
        let offset = |list: &[block::FrameOffset], slot, subslot, len: usize, limit: u16| {
            list.iter()
                .find(|o| (o.slot, o.subslot) == (slot, subslot))
                .map(|o| usize::from(o.offset))
                .filter(|at| at + len < usize::from(limit))
        };
        let mut spots = Vec::new();
        for p in &self.layout.places {
            let wanted = expected.iter().any(|m| {
                (m.api, m.slot) == (p.api, p.slot) && m.submodules.iter().any(|s| s.subslot == p.subslot)
            });
            let differs = diff.iter().any(|d| {
                (d.api, d.slot) == (p.api, p.slot)
                    && (d.state != block::MODULE_PROPER
                        || d.submodules.iter().any(|s| s.subslot == p.subslot))
            });
            if !wanted || differs {
                continue;
            }
            let find = |cr: &block::IocrReq, data: bool, len: u16| {
                cr.apis.iter().filter(|a| a.api == p.api).find_map(|a| {
                    offset(
                        if data { &a.data } else { &a.iocs },
                        p.slot,
                        p.subslot,
                        usize::from(len),
                        cr.data_length,
                    )
                })
            };
            spots.push(Spot {
                input_len: usize::from(p.input_len),
                output_len: usize::from(p.output_len),
                input_image: p.input_image,
                output_image: p.output_image,
                input_frame: if p.input_frame.is_some() {
                    find(input, true, p.input_len)
                } else {
                    None
                },
                input_iocs: if p.output_len > 0 {
                    find(input, false, 0)
                } else {
                    None
                },
                output_frame: if p.output_len > 0 {
                    find(output, true, p.output_len)
                } else {
                    None
                },
            });
        }
        // A submodule the controller expects but gave no place in a frame
        // is a faulty IOCR.
        let placed = spots.iter().all(|s| {
            (s.input_len == 0 && s.output_len > 0 || s.input_frame.is_some())
                && (s.output_len == 0 || (s.output_frame.is_some() && s.input_iocs.is_some()))
        });
        placed.then_some(spots)
    }

    fn on_connect(&mut self, now: u64, ip: Ip, blocks: &[u8]) -> (Status, Vec<u8>) {
        let Ok(req) = ConnectReq::decode(blocks) else {
            return (STATUS_FAULTY_IOCR, Vec::new());
        };
        let input = req.iocrs.iter().find(|c| c.iocr_type == block::IOCR_INPUT);
        let output = req.iocrs.iter().find(|c| c.iocr_type == block::IOCR_OUTPUT);
        let (Some(input), Some(output)) = (input, output) else {
            return (STATUS_FAULTY_IOCR, Vec::new());
        };
        // The checks a device makes of the timing and frame ids (p-net
        // `pf_cmdev_check_iocr_param`).
        let class1 = eth::FRAME_ID_RTC1_FIRST..=eth::FRAME_ID_RTC1_LAST;
        let bad = |c: &block::IocrReq| {
            !(rt::MIN_DATA..=rt::MAX_DATA).contains(&c.data_length)
                || !c.send_clock_factor.is_power_of_two()
                || c.send_clock_factor > 128
                || !(1..=512).contains(&c.reduction_ratio)
                || c.phase == 0
                || c.phase > c.reduction_ratio
                || c.data_hold_factor == 0
        };
        if bad(input)
            || bad(output)
            || !class1.contains(&input.frame_id)
            || (output.frame_id != block::FRAME_ID_ANY && !class1.contains(&output.frame_id))
        {
            return (STATUS_FAULTY_IOCR, Vec::new());
        }
        let expected = rt::modules_of(&req.expected);
        let diff = self.diff(&expected);
        let Some(spots) = self.spots(&expected, &diff, input, output) else {
            return (STATUS_FAULTY_IOCR, Vec::new());
        };
        let output_frame_id = if output.frame_id == block::FRAME_ID_ANY {
            OUTPUT_FRAME_ID
        } else {
            output.frame_id
        };
        let interval_us = rt::interval_us(input.send_clock_factor, input.reduction_ratio);
        let activity = Uuid::new(
            (self.random() >> 32) as u32,
            0x1234,
            0x4000,
            self.random().to_be_bytes(),
        );
        self.outputs.fill(0);
        self.connects += 1;
        self.ar = Some(Ar {
            uuid: req.ar.ar_uuid,
            session_key: req.ar.session_key,
            controller_mac: req.ar.initiator_mac,
            controller_ip: ip,
            controller_alarm_ref: req.alarm.local_alarm_ref,
            input_frame_id: input.frame_id,
            output_frame_id,
            input_len: usize::from(input.data_length),
            output_len: output.data_length,
            interval_us,
            hold_us: rt::interval_us(output.send_clock_factor, output.reduction_ratio)
                * u64::from(output.data_hold_factor),
            clock: (input.send_clock_factor, input.reduction_ratio),
            vlan: Some(Vlan::from_tci(input.tag)),
            spots,
            next_send: now,
            last_cycle: 0,
            last_output: None,
            ready_at: None,
            ready_activity: activity,
            ready_done: false,
            alarm_send: alarm::SEQ_FIRST,
            alarm_expect: alarm::SEQ_FIRST,
            alarm_taken: alarm::SEQ_NONE,
        });
        let res = ConnectRes {
            ar: ArRes {
                ar_type: req.ar.ar_type,
                ar_uuid: req.ar.ar_uuid,
                session_key: req.ar.session_key,
                responder_mac: self.mac,
                udp_rt_port: eth::ETHERTYPE_PROFINET,
            },
            iocrs: alloc::vec![
                IocrRes {
                    iocr_type: block::IOCR_INPUT,
                    reference: input.reference,
                    frame_id: input.frame_id,
                },
                IocrRes {
                    iocr_type: block::IOCR_OUTPUT,
                    reference: output.reference,
                    frame_id: output_frame_id,
                },
            ],
            alarm: AlarmCrRes {
                local_alarm_ref: ALARM_REF,
                max_alarm_data: req.alarm.max_alarm_data,
            },
            diff,
        };
        (Status::OK, res.encode())
    }

    fn on_write(&mut self, blocks: &[u8]) -> (Status, Vec<u8>) {
        let Ok(req) = WriteReq::decode(blocks) else {
            return (STATUS_INVALID_INDEX, Vec::new());
        };
        let known = self.ar.as_ref().is_some_and(|a| a.uuid == req.ar_uuid);
        let status = if !known || self.faults.refuse_record == Some(req.index) {
            STATUS_INVALID_INDEX
        } else {
            self.records.push((req.slot, req.subslot, req.index, req.data));
            Status::OK
        };
        let res = WriteRes {
            sequence: req.sequence,
            ar_uuid: req.ar_uuid,
            api: req.api,
            slot: req.slot,
            subslot: req.subslot,
            index: req.index,
            status,
        };
        (status, res.encode())
    }

    fn control_answer(req: &Control, block_type: u16) -> Vec<u8> {
        Control {
            block_type,
            ar_uuid: req.ar_uuid,
            session_key: req.session_key,
            command: block::COMMAND_DONE,
            diff: Vec::new(),
        }
        .encode()
    }

    fn on_control(&mut self, now: u64, blocks: &[u8]) -> (Status, Vec<u8>) {
        let unknown = Status {
            code: 0xDD,
            decode: 0x81,
            code1: 0x40,
            code2: 0x05,
        };
        let Ok(req) = Control::decode(blocks) else {
            return (unknown, Vec::new());
        };
        let Some(ar) = self.ar.as_mut().filter(|a| a.uuid == req.ar_uuid) else {
            return (unknown, Vec::new());
        };
        if req.block_type != block::PRM_END_REQ || req.command != block::COMMAND_PRM_END {
            return (unknown, Vec::new());
        }
        // A repeated PrmEnd is answered again and changes nothing.
        if ar.ready_at.is_none() {
            ar.ready_at = Some(now);
        }
        (Status::OK, Self::control_answer(&req, block::PRM_END_RES))
    }

    fn on_release(&mut self, blocks: &[u8]) -> (Status, Vec<u8>) {
        let Ok(req) = Control::decode(blocks) else {
            return (Status::OK, Vec::new());
        };
        if self.ar.as_ref().is_some_and(|a| a.uuid == req.ar_uuid) {
            self.drop_ar();
        }
        (Status::OK, Self::control_answer(&req, block::RELEASE_RES))
    }

    /// Raises a low-priority alarm on a submodule. Returns whether there
    /// was an AR to send it on.
    pub fn raise_alarm(
        &mut self,
        alarm_type: u16,
        slot: u16,
        subslot: u16,
        data: &[u8],
        out: &mut Vec<Out>,
    ) -> bool {
        let Some(ar) = &mut self.ar else { return false };
        let ident = |f: fn(&Module, u16) -> u32| {
            self.modules
                .iter()
                .find(|m| m.slot == slot)
                .map_or(0, |m| f(m, subslot))
        };
        let n = Notification {
            high: false,
            alarm_type,
            api: 0,
            slot,
            subslot,
            module_ident: ident(|m, _| m.ident),
            submodule_ident: ident(|m, sub| {
                m.submodules
                    .iter()
                    .find(|s| s.subslot == sub)
                    .map_or(0, |s| s.ident)
            }),
            specifier: 0,
            data: data.into(),
        };
        let pdu = Rta {
            dst_ref: ar.controller_alarm_ref,
            src_ref: ALARM_REF,
            pdu_type: alarm::PDU_DATA,
            tack: true,
            send_seq: ar.alarm_send,
            ack_seq: ar.alarm_taken,
            var: n.encode(),
        };
        out.push(Out::Frame(eth::encode(
            ar.controller_mac,
            self.mac,
            Some(Vlan { priority: 5, id: 0 }),
            eth::FRAME_ID_ALARM_LOW,
            &pdu.encode(),
        )));
        true
    }

    fn on_alarm(&mut self, f: &eth::Frame<'_>, out: &mut Vec<Out>) {
        let Some(ar) = &mut self.ar else { return };
        let Ok(rta) = Rta::parse(f.payload) else { return };
        if f.src != ar.controller_mac || rta.dst_ref != ALARM_REF {
            return;
        }
        match rta.pdu_type {
            alarm::PDU_ACK if rta.ack_seq == ar.alarm_send => {
                ar.alarm_send = alarm::next_seq(ar.alarm_send);
            }
            alarm::PDU_DATA => {
                let fresh = rta.send_seq == ar.alarm_expect;
                if !fresh && rta.send_seq != ar.alarm_taken {
                    return;
                }
                if fresh {
                    ar.alarm_taken = ar.alarm_expect;
                    ar.alarm_expect = alarm::next_seq(ar.alarm_expect);
                    if Ack::parse(&rta.var).is_ok_and(|a| a.status.is_ok()) {
                        self.alarms_acked += 1;
                    }
                }
                let pdu = Rta {
                    dst_ref: ar.controller_alarm_ref,
                    src_ref: ALARM_REF,
                    pdu_type: alarm::PDU_ACK,
                    tack: false,
                    send_seq: alarm::SEQ_NONE,
                    ack_seq: ar.alarm_taken,
                    var: Vec::new(),
                };
                out.push(Out::Frame(eth::encode(
                    f.src,
                    self.mac,
                    f.vlan,
                    f.frame_id,
                    &pdu.encode(),
                )));
            }
            _ => {}
        }
    }
}

/// A station description for a simulated device.
#[must_use]
pub fn station(name: &str, vendor_id: u16, device_id: u16, ip: Ip) -> Station {
    Station {
        name: String::from(name),
        vendor_id,
        device_id,
        role: dcp::ROLE_DEVICE,
        type_of_station: String::from("helmsman-sim"),
        ip,
        mask: if ip == [0; 4] { [0; 4] } else { [255, 255, 255, 0] },
        gateway: [0; 4],
        instance: Some(1),
    }
}
