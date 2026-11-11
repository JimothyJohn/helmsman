//! The blocks of the context-management PDUs: what a Connect request and
//! its response carry, the control blocks that end parameterisation and
//! announce "application ready", record writes, and Release.
//!
//! Every block is type, length, version (1.0) and a body, all big-endian;
//! the length counts the version bytes and the body (p-net
//! `pf_block_writer.c` `pf_put_block_header`: "Always (-4)"). Field order
//! as p-net reads requests (`pf_block_reader.c`: `pf_get_ar_param`,
//! `pf_get_iocr_param`, `pf_get_alarm_cr_request`, `pf_get_exp_api_module`,
//! `pf_get_control`, `pf_get_write_request`) and writes responses
//! (`pf_block_writer.c`: `pf_put_ar_result`, `pf_put_iocr_result`,
//! `pf_put_alarm_cr_result`, `pf_put_ar_diff`, `pf_put_control`,
//! `pf_put_write_result`); block type numbers from its `pf_types.h`
//! `PF_BT_*`, which agree with Wireshark's `packet-dcerpc-pn-io.c`.

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::{DecodeError, Reader, Writer, len16};
use crate::eth::{ETHERTYPE_PROFINET, Mac};
use crate::rpc::{Status, Uuid};

pub const AR_BLOCK_REQ: u16 = 0x0101;
pub const IOCR_BLOCK_REQ: u16 = 0x0102;
pub const ALARM_CR_BLOCK_REQ: u16 = 0x0103;
pub const EXPECTED_SUBMODULE_BLOCK_REQ: u16 = 0x0104;
pub const AR_BLOCK_RES: u16 = 0x8101;
pub const IOCR_BLOCK_RES: u16 = 0x8102;
pub const ALARM_CR_BLOCK_RES: u16 = 0x8103;
pub const MODULE_DIFF_BLOCK: u16 = 0x8104;
pub const PRM_END_REQ: u16 = 0x0110;
pub const PRM_END_RES: u16 = 0x8110;
pub const APPLICATION_READY_REQ: u16 = 0x0112;
pub const APPLICATION_READY_RES: u16 = 0x8112;
pub const RELEASE_REQ: u16 = 0x0114;
pub const RELEASE_RES: u16 = 0x8114;
pub const WRITE_REQ: u16 = 0x0008;
pub const WRITE_RES: u16 = 0x8008;

/// AR type: a single IO controller AR (p-net `PF_ART_IOCAR_SINGLE`).
pub const AR_TYPE_SINGLE: u16 = 0x0001;
/// AR properties: state active (bits 0..=2 = 1) and "parameterisation by
/// the CM initiator" (bit 4), the bits p-net's `pf_get_ar_param` names.
pub const AR_PROPERTIES_ACTIVE: u32 = 0x0000_0011;
/// AR properties bit 30: the advanced startup mode.
pub const AR_PROPERTIES_STARTUP_ADVANCED: u32 = 0x4000_0000;

/// IOCR types (p-net `PF_IOCR_TYPE_*`).
pub const IOCR_INPUT: u16 = 1;
pub const IOCR_OUTPUT: u16 = 2;
/// IOCR properties: RT class 1 in the low four bits (p-net
/// `PF_RT_CLASS_1`; class 2 is what current engineering tools write for
/// plain RT too).
pub const RT_CLASS_1: u32 = 1;
pub const RT_CLASS_2: u32 = 2;
/// "The device picks the frame id", in an output CR's request (p-net
/// `pf_cmdev_fix_frame_id`).
pub const FRAME_ID_ANY: u16 = 0xFFFF;

/// Control commands, one bit each (p-net `PF_CONTROL_COMMAND_BIT_*`).
pub const COMMAND_PRM_END: u16 = 0x0001;
pub const COMMAND_APPLICATION_READY: u16 = 0x0002;
pub const COMMAND_RELEASE: u16 = 0x0004;
pub const COMMAND_DONE: u16 = 0x0008;

/// Submodule data directions in a data description (p-net
/// `PF_DIRECTION_*`).
pub const DIRECTION_INPUT: u16 = 1;
pub const DIRECTION_OUTPUT: u16 = 2;

/// Module states of a `ModuleDiffBlock`.
pub const MODULE_NONE: u16 = 0;
pub const MODULE_WRONG: u16 = 1;
pub const MODULE_PROPER: u16 = 2;
pub const MODULE_SUBSTITUTE: u16 = 3;

/// Writes one block: header, then whatever `body` writes.
fn put(w: &mut Writer, block_type: u16, body: impl FnOnce(&mut Writer)) {
    w.u16(block_type);
    let at = w.len();
    w.u16(0).u8(1).u8(0);
    body(w);
    // A block is bounded by the RPC body (rpc::MAX_BODY) well below 64 KiB.
    w.set_u16(at, u16::try_from(w.len() - at - 2).unwrap_or(u16::MAX));
}

/// One block as read: its type and a reader over its body.
pub struct Block<'a> {
    pub block_type: u16,
    pub body: Reader<'a>,
}

/// Takes the next block off `r`.
///
/// # Errors
/// Truncated, or a length too short to hold the version.
pub fn next<'a>(r: &mut Reader<'a>) -> Result<Block<'a>, DecodeError> {
    let block_type = r.u16()?;
    let len = usize::from(r.u16()?);
    if len < 2 {
        return Err(DecodeError::Invalid("block shorter than its version"));
    }
    let mut body = r.sub(len)?;
    body.bytes(2)?;
    Ok(Block { block_type, body })
}

/// The `ARBlockReq`: who is connecting and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArReq {
    pub ar_type: u16,
    pub ar_uuid: Uuid,
    pub session_key: u16,
    pub initiator_mac: Mac,
    pub initiator_object: Uuid,
    pub properties: u32,
    /// In units of 100 ms, 1..=1000 (p-net `pf_cmdev.c`): how long the
    /// device waits for the next step of the startup.
    pub activity_timeout: u16,
    /// The controller's port for RT over UDP, 0x8892; unused by RT class 1
    /// but always present.
    pub udp_rt_port: u16,
    pub station_name: String,
}

/// One submodule's place in a cyclic frame: its data (with the provider
/// status after it) or its consumer status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameOffset {
    pub slot: u16,
    pub subslot: u16,
    pub offset: u16,
}

/// The part of an IOCR that belongs to one API.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IocrApi {
    pub api: u32,
    /// Data objects this frame carries, each followed by its IOPS.
    pub data: Vec<FrameOffset>,
    /// Consumer statuses this frame carries, for data going the other way.
    pub iocs: Vec<FrameOffset>,
}

/// The `IOCRBlockReq`: one direction of cyclic data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IocrReq {
    pub iocr_type: u16,
    pub reference: u16,
    pub properties: u32,
    /// The length of the frame's data part (`C_SDU`), 40..=1440 for RT
    /// class 1 (p-net `pf_cmdev_check_iocr_param`).
    pub data_length: u16,
    pub frame_id: u16,
    /// The base clock in units of 31.25 us; 32 is 1 ms.
    pub send_clock_factor: u16,
    /// A frame every `reduction_ratio` base clocks.
    pub reduction_ratio: u16,
    pub phase: u16,
    pub frame_send_offset: u32,
    pub watchdog_factor: u16,
    /// The consumer gives up after this many missed cycles.
    pub data_hold_factor: u16,
    /// The 802.1Q tag control word the frames carry.
    pub tag: u16,
    pub multicast_mac: Mac,
    pub apis: Vec<IocrApi>,
}

/// The `AlarmCRBlockReq`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlarmCrReq {
    pub properties: u32,
    /// In units of 100 ms, 1..=100 (p-net `pf_cmdev.c`).
    pub rta_timeout_factor: u16,
    /// 3..=15.
    pub rta_retries: u16,
    pub local_alarm_ref: u16,
    /// 200..=1432.
    pub max_alarm_data: u16,
    pub tag_high: u16,
    pub tag_low: u16,
}

/// What a submodule's data looks like in one direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataDescription {
    pub direction: u16,
    pub length: u16,
    pub iocs_length: u8,
    pub iops_length: u8,
}

/// One expected submodule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedSubmodule {
    pub subslot: u16,
    pub ident: u32,
    /// Bits 0..=1: 0 no data, 1 input, 2 output, 3 both.
    pub properties: u16,
    /// One description, or two (input then output) for a submodule with
    /// both.
    pub data: Vec<DataDescription>,
}

/// One expected module: an entry of the `ExpectedSubmoduleBlockReq`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedModule {
    pub api: u32,
    pub slot: u16,
    pub ident: u32,
    pub properties: u16,
    pub submodules: Vec<ExpectedSubmodule>,
}

/// A Connect request's blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectReq {
    pub ar: ArReq,
    pub iocrs: Vec<IocrReq>,
    pub alarm: AlarmCrReq,
    pub expected: Vec<ExpectedModule>,
}

fn put_offsets(w: &mut Writer, list: &[FrameOffset]) -> Result<(), DecodeError> {
    w.u16(len16(list.len(), "too many frame offsets")?);
    for o in list {
        w.u16(o.slot).u16(o.subslot).u16(o.offset);
    }
    Ok(())
}

fn get_offsets(r: &mut Reader<'_>) -> Result<Vec<FrameOffset>, DecodeError> {
    let n = r.u16()?;
    let mut list = Vec::new();
    for _ in 0..n {
        list.push(FrameOffset {
            slot: r.u16()?,
            subslot: r.u16()?,
            offset: r.u16()?,
        });
    }
    Ok(list)
}

impl ConnectReq {
    /// The blocks, in the order a controller sends them: AR, IOCRs,
    /// alarm CR, expected submodules.
    ///
    /// # Errors
    /// A count or the station name too long for its field.
    pub fn encode(&self) -> Result<Vec<u8>, DecodeError> {
        let mut w = Writer::with_capacity(512);
        let ar = &self.ar;
        let name_len = len16(ar.station_name.len(), "station name too long")?;
        put(&mut w, AR_BLOCK_REQ, |w| {
            w.u16(ar.ar_type).bytes(&ar.ar_uuid.0).u16(ar.session_key);
            w.bytes(&ar.initiator_mac).bytes(&ar.initiator_object.0);
            w.u32(ar.properties).u16(ar.activity_timeout).u16(ar.udp_rt_port);
            w.u16(name_len).bytes(ar.station_name.as_bytes());
        });
        for cr in &self.iocrs {
            let mut result = Ok(());
            put(&mut w, IOCR_BLOCK_REQ, |w| {
                result = (|| {
                    w.u16(cr.iocr_type).u16(cr.reference).u16(ETHERTYPE_PROFINET);
                    w.u32(cr.properties).u16(cr.data_length).u16(cr.frame_id);
                    w.u16(cr.send_clock_factor).u16(cr.reduction_ratio).u16(cr.phase);
                    // Sequence, unused (0).
                    w.u16(0).u32(cr.frame_send_offset);
                    w.u16(cr.watchdog_factor).u16(cr.data_hold_factor).u16(cr.tag);
                    w.bytes(&cr.multicast_mac);
                    w.u16(len16(cr.apis.len(), "too many APIs")?);
                    for api in &cr.apis {
                        w.u32(api.api);
                        put_offsets(w, &api.data)?;
                        put_offsets(w, &api.iocs)?;
                    }
                    Ok(())
                })();
            });
            result?;
        }
        let al = &self.alarm;
        put(&mut w, ALARM_CR_BLOCK_REQ, |w| {
            // AlarmCRType 1, LT 0x8892.
            w.u16(1).u16(ETHERTYPE_PROFINET).u32(al.properties);
            w.u16(al.rta_timeout_factor)
                .u16(al.rta_retries)
                .u16(al.local_alarm_ref);
            w.u16(al.max_alarm_data).u16(al.tag_high).u16(al.tag_low);
        });
        let mut result = Ok(());
        put(&mut w, EXPECTED_SUBMODULE_BLOCK_REQ, |w| {
            result = (|| {
                w.u16(len16(self.expected.len(), "too many modules")?);
                for m in &self.expected {
                    w.u32(m.api).u16(m.slot).u32(m.ident).u16(m.properties);
                    w.u16(len16(m.submodules.len(), "too many submodules")?);
                    for s in &m.submodules {
                        w.u16(s.subslot).u32(s.ident).u16(s.properties);
                        for d in &s.data {
                            w.u16(d.direction)
                                .u16(d.length)
                                .u8(d.iocs_length)
                                .u8(d.iops_length);
                        }
                    }
                }
                Ok(())
            })();
        });
        result?;
        Ok(w.into_inner())
    }

    /// Reads a Connect request's blocks, as a device does. Blocks of
    /// other types are skipped.
    ///
    /// # Errors
    /// Malformed, or without an AR block or an alarm CR block.
    pub fn decode(blocks: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(blocks);
        let mut ar = None;
        let mut iocrs = Vec::new();
        let mut alarm = None;
        let mut expected = Vec::new();
        while !r.is_empty() {
            let Block { block_type, mut body } = next(&mut r)?;
            let b = &mut body;
            match block_type {
                AR_BLOCK_REQ => {
                    let ar_type = b.u16()?;
                    let ar_uuid = Uuid(b.array()?);
                    let session_key = b.u16()?;
                    let initiator_mac = b.array()?;
                    let initiator_object = Uuid(b.array()?);
                    let properties = b.u32()?;
                    let activity_timeout = b.u16()?;
                    let udp_rt_port = b.u16()?;
                    let name_len = usize::from(b.u16()?);
                    let station_name = core::str::from_utf8(b.bytes(name_len)?)
                        .map_err(|_| DecodeError::Invalid("station name is not UTF-8"))?
                        .into();
                    ar = Some(ArReq {
                        ar_type,
                        ar_uuid,
                        session_key,
                        initiator_mac,
                        initiator_object,
                        properties,
                        activity_timeout,
                        udp_rt_port,
                        station_name,
                    });
                }
                IOCR_BLOCK_REQ => {
                    let iocr_type = b.u16()?;
                    let reference = b.u16()?;
                    b.u16()?;
                    let properties = b.u32()?;
                    let data_length = b.u16()?;
                    let frame_id = b.u16()?;
                    let send_clock_factor = b.u16()?;
                    let reduction_ratio = b.u16()?;
                    let phase = b.u16()?;
                    b.u16()?;
                    let frame_send_offset = b.u32()?;
                    let watchdog_factor = b.u16()?;
                    let data_hold_factor = b.u16()?;
                    let tag = b.u16()?;
                    let multicast_mac = b.array()?;
                    let mut apis = Vec::new();
                    for _ in 0..b.u16()? {
                        apis.push(IocrApi {
                            api: b.u32()?,
                            data: get_offsets(b)?,
                            iocs: get_offsets(b)?,
                        });
                    }
                    iocrs.push(IocrReq {
                        iocr_type,
                        reference,
                        properties,
                        data_length,
                        frame_id,
                        send_clock_factor,
                        reduction_ratio,
                        phase,
                        frame_send_offset,
                        watchdog_factor,
                        data_hold_factor,
                        tag,
                        multicast_mac,
                        apis,
                    });
                }
                ALARM_CR_BLOCK_REQ => {
                    b.u16()?;
                    b.u16()?;
                    alarm = Some(AlarmCrReq {
                        properties: b.u32()?,
                        rta_timeout_factor: b.u16()?,
                        rta_retries: b.u16()?,
                        local_alarm_ref: b.u16()?,
                        max_alarm_data: b.u16()?,
                        tag_high: b.u16()?,
                        tag_low: b.u16()?,
                    });
                }
                EXPECTED_SUBMODULE_BLOCK_REQ => {
                    for _ in 0..b.u16()? {
                        let api = b.u32()?;
                        let slot = b.u16()?;
                        let ident = b.u32()?;
                        let properties = b.u16()?;
                        let mut submodules = Vec::new();
                        for _ in 0..b.u16()? {
                            let subslot = b.u16()?;
                            let ident = b.u32()?;
                            let properties = b.u16()?;
                            // Two descriptions when the submodule has
                            // both directions (p-net `pf_get_exp_submodule`).
                            let n = if properties & 3 == 3 { 2 } else { 1 };
                            let mut data = Vec::new();
                            for _ in 0..n {
                                data.push(DataDescription {
                                    direction: b.u16()?,
                                    length: b.u16()?,
                                    iocs_length: b.u8()?,
                                    iops_length: b.u8()?,
                                });
                            }
                            submodules.push(ExpectedSubmodule {
                                subslot,
                                ident,
                                properties,
                                data,
                            });
                        }
                        expected.push(ExpectedModule {
                            api,
                            slot,
                            ident,
                            properties,
                            submodules,
                        });
                    }
                }
                _ => {}
            }
        }
        Ok(Self {
            ar: ar.ok_or(DecodeError::Invalid("Connect without an AR block"))?,
            iocrs,
            alarm: alarm.ok_or(DecodeError::Invalid("Connect without an alarm CR block"))?,
            expected,
        })
    }
}

/// The `ARBlockRes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArRes {
    pub ar_type: u16,
    pub ar_uuid: Uuid,
    pub session_key: u16,
    pub responder_mac: Mac,
    pub udp_rt_port: u16,
}

/// The `IOCRBlockRes`: the frame id that direction will use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IocrRes {
    pub iocr_type: u16,
    pub reference: u16,
    pub frame_id: u16,
}

/// The `AlarmCRBlockRes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlarmCrRes {
    pub local_alarm_ref: u16,
    pub max_alarm_data: u16,
}

/// A submodule that is not what was expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubmoduleDiff {
    pub subslot: u16,
    pub ident: u32,
    /// Bit 15 set (the format indicator); bits 11..=14 the identification
    /// (0 as expected, 1 a substitute, 2 wrong, 3 none); bits 7..=10 the
    /// AR's hold on it (0 owned, 1 application ready pending, 2 and up
    /// locked by another); as p-net's `pf_put_submodule_diff` packs it.
    pub state: u16,
}

impl SubmoduleDiff {
    /// The state word of a submodule that is not there.
    pub const NO_SUBMODULE: u16 = 0x8000 | (3 << 11);
    /// The state word of a submodule of another type than expected.
    pub const WRONG: u16 = 0x8000 | (2 << 11);

    /// The identification field: 0 as expected, 1 a substitute, 2 wrong,
    /// 3 no submodule.
    #[must_use]
    pub const fn ident_info(self) -> u16 {
        (self.state >> 11) & 0xF
    }

    /// The AR's hold on it: 0 owned, anything else not this AR's (yet).
    #[must_use]
    pub const fn ar_info(self) -> u16 {
        (self.state >> 7) & 0xF
    }
}

/// A module with something that differs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleDiff {
    pub api: u32,
    pub slot: u16,
    pub ident: u32,
    /// [`MODULE_NONE`] .. [`MODULE_SUBSTITUTE`].
    pub state: u16,
    pub submodules: Vec<SubmoduleDiff>,
}

fn put_diff(w: &mut Writer, diff: &[ModuleDiff]) {
    if diff.is_empty() {
        return;
    }
    put(w, MODULE_DIFF_BLOCK, |w| {
        // One API entry per module: simple, and what a reader must take.
        w.u16(u16::try_from(diff.len()).unwrap_or(u16::MAX));
        for m in diff {
            w.u32(m.api).u16(1).u16(m.slot).u32(m.ident).u16(m.state);
            w.u16(u16::try_from(m.submodules.len()).unwrap_or(u16::MAX));
            for s in &m.submodules {
                w.u16(s.subslot).u32(s.ident).u16(s.state);
            }
        }
    });
}

fn get_diff(b: &mut Reader<'_>, out: &mut Vec<ModuleDiff>) -> Result<(), DecodeError> {
    for _ in 0..b.u16()? {
        let api = b.u32()?;
        for _ in 0..b.u16()? {
            let slot = b.u16()?;
            let ident = b.u32()?;
            let state = b.u16()?;
            let mut submodules = Vec::new();
            for _ in 0..b.u16()? {
                submodules.push(SubmoduleDiff {
                    subslot: b.u16()?,
                    ident: b.u32()?,
                    state: b.u16()?,
                });
            }
            out.push(ModuleDiff {
                api,
                slot,
                ident,
                state,
                submodules,
            });
        }
    }
    Ok(())
}

/// A Connect response's blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectRes {
    pub ar: ArRes,
    pub iocrs: Vec<IocrRes>,
    pub alarm: AlarmCrRes,
    /// Empty when the device holds exactly what was expected.
    pub diff: Vec<ModuleDiff>,
}

impl ConnectRes {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(128);
        let ar = &self.ar;
        put(&mut w, AR_BLOCK_RES, |w| {
            w.u16(ar.ar_type).bytes(&ar.ar_uuid.0).u16(ar.session_key);
            w.bytes(&ar.responder_mac).u16(ar.udp_rt_port);
        });
        for cr in &self.iocrs {
            put(&mut w, IOCR_BLOCK_RES, |w| {
                w.u16(cr.iocr_type).u16(cr.reference).u16(cr.frame_id);
            });
        }
        put(&mut w, ALARM_CR_BLOCK_RES, |w| {
            w.u16(1)
                .u16(self.alarm.local_alarm_ref)
                .u16(self.alarm.max_alarm_data);
        });
        put_diff(&mut w, &self.diff);
        w.into_inner()
    }

    /// Reads a Connect response's blocks. Blocks of other types (the
    /// `ARServerBlock` of the advanced startup, vendor blocks) are
    /// skipped.
    ///
    /// # Errors
    /// Malformed, or without an AR block or an alarm CR block.
    pub fn decode(blocks: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(blocks);
        let mut ar = None;
        let mut iocrs = Vec::new();
        let mut alarm = None;
        let mut diff = Vec::new();
        while !r.is_empty() {
            let Block { block_type, mut body } = next(&mut r)?;
            let b = &mut body;
            match block_type {
                AR_BLOCK_RES => {
                    ar = Some(ArRes {
                        ar_type: b.u16()?,
                        ar_uuid: Uuid(b.array()?),
                        session_key: b.u16()?,
                        responder_mac: b.array()?,
                        udp_rt_port: b.u16()?,
                    });
                }
                IOCR_BLOCK_RES => iocrs.push(IocrRes {
                    iocr_type: b.u16()?,
                    reference: b.u16()?,
                    frame_id: b.u16()?,
                }),
                ALARM_CR_BLOCK_RES => {
                    b.u16()?;
                    alarm = Some(AlarmCrRes {
                        local_alarm_ref: b.u16()?,
                        max_alarm_data: b.u16()?,
                    });
                }
                MODULE_DIFF_BLOCK => get_diff(b, &mut diff)?,
                _ => {}
            }
        }
        Ok(Self {
            ar: ar.ok_or(DecodeError::Invalid("Connect response without an AR block"))?,
            iocrs,
            alarm: alarm.ok_or(DecodeError::Invalid("Connect response without an alarm CR block"))?,
            diff,
        })
    }
}

/// A control block: `PrmEnd`, "application ready", Release, and their
/// responses, which share one layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Control {
    pub block_type: u16,
    pub ar_uuid: Uuid,
    pub session_key: u16,
    /// [`COMMAND_PRM_END`] and friends; a response says [`COMMAND_DONE`].
    pub command: u16,
    /// Submodules still not as expected, in an "application ready".
    pub diff: Vec<ModuleDiff>,
}

impl Control {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(40);
        put(&mut w, self.block_type, |w| {
            // Two reserved bytes, the AR, the session key, two more
            // reserved (an alarm sequence number in the plug variants),
            // the command and the properties (0).
            w.u16(0).bytes(&self.ar_uuid.0).u16(self.session_key);
            w.u16(0).u16(self.command).u16(0);
        });
        put_diff(&mut w, &self.diff);
        w.into_inner()
    }

    /// Reads a control PDU's blocks: the control block, and a
    /// `ModuleDiffBlock` when one follows.
    ///
    /// # Errors
    /// Malformed, or no control block first.
    pub fn decode(blocks: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(blocks);
        let Block { block_type, mut body } = next(&mut r)?;
        if !matches!(
            block_type,
            PRM_END_REQ
                | PRM_END_RES
                | APPLICATION_READY_REQ
                | APPLICATION_READY_RES
                | RELEASE_REQ
                | RELEASE_RES
        ) {
            return Err(DecodeError::Invalid("not a control block"));
        }
        body.u16()?;
        let ar_uuid = Uuid(body.array()?);
        let session_key = body.u16()?;
        body.u16()?;
        let command = body.u16()?;
        let mut diff = Vec::new();
        while !r.is_empty() {
            let Block { block_type, mut body } = next(&mut r)?;
            if block_type == MODULE_DIFF_BLOCK {
                get_diff(&mut body, &mut diff)?;
            }
        }
        Ok(Self {
            block_type,
            ar_uuid,
            session_key,
            command,
            diff,
        })
    }
}

/// A record write: the `IODWriteReqHeader` and the record's bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteReq {
    pub sequence: u16,
    pub ar_uuid: Uuid,
    pub api: u32,
    pub slot: u16,
    pub subslot: u16,
    pub index: u16,
    pub data: Vec<u8>,
}

impl WriteReq {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(64 + self.data.len());
        put(&mut w, WRITE_REQ, |w| {
            w.u16(self.sequence).bytes(&self.ar_uuid.0).u32(self.api);
            w.u16(self.slot).u16(self.subslot).u16(0).u16(self.index);
            w.u32(u32::try_from(self.data.len()).unwrap_or(u32::MAX));
            // RWPadding: the header is 64 bytes whatever it carries.
            w.zeros(24);
        });
        w.bytes(&self.data);
        w.into_inner()
    }

    /// # Errors
    /// Malformed, not a write header, or less data than it says.
    pub fn decode(blocks: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(blocks);
        let Block { block_type, mut body } = next(&mut r)?;
        if block_type != WRITE_REQ {
            return Err(DecodeError::Invalid("not a write request"));
        }
        let sequence = body.u16()?;
        let ar_uuid = Uuid(body.array()?);
        let api = body.u32()?;
        let slot = body.u16()?;
        let subslot = body.u16()?;
        body.u16()?;
        let index = body.u16()?;
        let len = usize::try_from(body.u32()?).map_err(|_| DecodeError::Invalid("record length"))?;
        Ok(Self {
            sequence,
            ar_uuid,
            api,
            slot,
            subslot,
            index,
            data: r.bytes(len)?.into(),
        })
    }
}

/// The `IODWriteResHeader`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteRes {
    pub sequence: u16,
    pub ar_uuid: Uuid,
    pub api: u32,
    pub slot: u16,
    pub subslot: u16,
    pub index: u16,
    pub status: Status,
}

impl WriteRes {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(64);
        put(&mut w, WRITE_RES, |w| {
            w.u16(self.sequence).bytes(&self.ar_uuid.0).u32(self.api);
            w.u16(self.slot).u16(self.subslot).u16(0).u16(self.index);
            // Record length 0, the two additional values, the status,
            // then padding to 64 bytes.
            w.u32(0).u16(0).u16(0).u32(self.status.to_u32()).zeros(16);
        });
        w.into_inner()
    }

    /// # Errors
    /// Malformed or not a write response header.
    pub fn decode(blocks: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(blocks);
        let Block { block_type, mut body } = next(&mut r)?;
        if block_type != WRITE_RES {
            return Err(DecodeError::Invalid("not a write response"));
        }
        let sequence = body.u16()?;
        let ar_uuid = Uuid(body.array()?);
        let api = body.u32()?;
        let slot = body.u16()?;
        let subslot = body.u16()?;
        body.u16()?;
        let index = body.u16()?;
        body.u32()?;
        body.u32()?;
        Ok(Self {
            sequence,
            ar_uuid,
            api,
            slot,
            subslot,
            index,
            status: Status::from_u32(body.u32()?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AR: Uuid = Uuid([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);

    fn connect() -> ConnectReq {
        ConnectReq {
            ar: ArReq {
                ar_type: AR_TYPE_SINGLE,
                ar_uuid: AR,
                session_key: 1,
                initiator_mac: [2, 0, 0, 0, 0, 1],
                initiator_object: crate::rpc::object_uuid(1, 1, 0x1234),
                properties: AR_PROPERTIES_ACTIVE,
                activity_timeout: 100,
                udp_rt_port: 0x8892,
                station_name: "helmsman".into(),
            },
            iocrs: alloc::vec![IocrReq {
                iocr_type: IOCR_INPUT,
                reference: 1,
                properties: RT_CLASS_1,
                data_length: 40,
                frame_id: 0xC000,
                send_clock_factor: 32,
                reduction_ratio: 8,
                phase: 1,
                frame_send_offset: 0xFFFF_FFFF,
                watchdog_factor: 3,
                data_hold_factor: 3,
                tag: 0xC000,
                multicast_mac: [0; 6],
                apis: alloc::vec![IocrApi {
                    api: 0,
                    data: alloc::vec![FrameOffset {
                        slot: 1,
                        subslot: 1,
                        offset: 0
                    }],
                    iocs: alloc::vec![],
                }],
            }],
            alarm: AlarmCrReq {
                properties: 0,
                rta_timeout_factor: 1,
                rta_retries: 3,
                local_alarm_ref: 1,
                max_alarm_data: 200,
                tag_high: 0xC000,
                tag_low: 0xA000,
            },
            expected: alloc::vec![ExpectedModule {
                api: 0,
                slot: 1,
                ident: 0x30,
                properties: 0,
                submodules: alloc::vec![
                    ExpectedSubmodule {
                        subslot: 1,
                        ident: 0x130,
                        properties: 1,
                        data: alloc::vec![DataDescription {
                            direction: DIRECTION_INPUT,
                            length: 2,
                            iocs_length: 1,
                            iops_length: 1
                        }],
                    },
                    ExpectedSubmodule {
                        subslot: 2,
                        ident: 0x131,
                        properties: 3,
                        data: alloc::vec![
                            DataDescription {
                                direction: DIRECTION_INPUT,
                                length: 1,
                                iocs_length: 1,
                                iops_length: 1
                            },
                            DataDescription {
                                direction: DIRECTION_OUTPUT,
                                length: 1,
                                iocs_length: 1,
                                iops_length: 1
                            }
                        ],
                    }
                ],
            }],
        }
    }

    #[test]
    fn a_connect_request_reads_back() {
        let req = connect();
        let bytes = req.encode().unwrap();
        assert_eq!(ConnectReq::decode(&bytes).unwrap(), req);
    }

    #[test]
    fn the_ar_block_is_the_bytes_on_the_wire() {
        let bytes = connect().encode().unwrap();
        // Type 0x0101; length = 2 (version) + 2 + 16 + 2 + 6 + 16 + 4 + 2 + 2
        // + 2 + 8 ("helmsman") = 62; version 1.0; AR type 1.
        assert_eq!(&bytes[..8], &[0x01, 0x01, 0x00, 62, 1, 0, 0x00, 0x01]);
        assert_eq!(&bytes[8..24], &AR.0);
        // Properties, timeout 100, port 0x8892, name length 8.
        assert_eq!(&bytes[48..58], &[0, 0, 0, 0x11, 0, 100, 0x88, 0x92, 0, 8]);
        assert_eq!(&bytes[58..66], b"helmsman");
        // The IOCR block follows: type 0x0102, CR type 1, reference 1, LT.
        assert_eq!(&bytes[66..68], &[0x01, 0x02]);
        assert_eq!(&bytes[72..78], &[0, 1, 0, 1, 0x88, 0x92]);
    }

    #[test]
    fn a_connect_response_reads_back_with_and_without_a_diff() {
        let mut res = ConnectRes {
            ar: ArRes {
                ar_type: AR_TYPE_SINGLE,
                ar_uuid: AR,
                session_key: 1,
                responder_mac: [2, 0, 0, 0, 0, 2],
                udp_rt_port: 0x8892,
            },
            iocrs: alloc::vec![
                IocrRes {
                    iocr_type: IOCR_INPUT,
                    reference: 1,
                    frame_id: 0xC000
                },
                IocrRes {
                    iocr_type: IOCR_OUTPUT,
                    reference: 2,
                    frame_id: 0xC001
                }
            ],
            alarm: AlarmCrRes {
                local_alarm_ref: 7,
                max_alarm_data: 200,
            },
            diff: alloc::vec![],
        };
        let bytes = res.encode();
        // ARBlockRes: 2 + 2 + 16 + 2 + 6 + 2 = 30.
        assert_eq!(&bytes[..6], &[0x81, 0x01, 0, 30, 1, 0]);
        assert_eq!(ConnectRes::decode(&bytes).unwrap(), res);
        res.diff = alloc::vec![ModuleDiff {
            api: 0,
            slot: 1,
            ident: 0x30,
            state: MODULE_PROPER,
            submodules: alloc::vec![SubmoduleDiff {
                subslot: 2,
                ident: 0,
                state: SubmoduleDiff::NO_SUBMODULE
            }],
        }];
        let back = ConnectRes::decode(&res.encode()).unwrap();
        assert_eq!(back, res);
        assert_eq!(back.diff[0].submodules[0].ident_info(), 3);
    }

    #[test]
    fn unknown_blocks_in_a_response_are_skipped() {
        let res = ConnectRes {
            ar: ArRes {
                ar_type: 1,
                ar_uuid: AR,
                session_key: 1,
                responder_mac: [0; 6],
                udp_rt_port: 0,
            },
            iocrs: alloc::vec![],
            alarm: AlarmCrRes {
                local_alarm_ref: 1,
                max_alarm_data: 200,
            },
            diff: alloc::vec![],
        };
        let mut bytes = res.encode();
        // An ARServerBlock (0x8106) the advanced startup adds.
        bytes.extend_from_slice(&[0x81, 0x06, 0, 6, 1, 0, 0, 2, b'x', b'y']);
        assert_eq!(ConnectRes::decode(&bytes).unwrap(), res);
    }

    #[test]
    fn a_control_block_is_the_bytes_on_the_wire() {
        let c = Control {
            block_type: PRM_END_REQ,
            ar_uuid: AR,
            session_key: 1,
            command: COMMAND_PRM_END,
            diff: alloc::vec![],
        };
        let bytes = c.encode();
        // Length 2 + 2 + 16 + 2 + 2 + 2 + 2 = 28: a 32-byte block.
        assert_eq!(bytes.len(), 32);
        assert_eq!(&bytes[..8], &[0x01, 0x10, 0, 28, 1, 0, 0, 0]);
        assert_eq!(&bytes[24..], &[0, 1, 0, 0, 0, 1, 0, 0]);
        assert_eq!(Control::decode(&bytes).unwrap(), c);
        assert!(Control::decode(&connect().encode().unwrap()).is_err());
    }

    #[test]
    fn a_write_is_a_64_byte_header_and_the_record() {
        let wr = WriteReq {
            sequence: 2,
            ar_uuid: AR,
            api: 0,
            slot: 1,
            subslot: 1,
            index: 0x0064,
            data: alloc::vec![9, 8, 7],
        };
        let bytes = wr.encode();
        assert_eq!(bytes.len(), 64 + 3);
        assert_eq!(&bytes[..6], &[0x00, 0x08, 0, 60, 1, 0]);
        assert_eq!(WriteReq::decode(&bytes).unwrap(), wr);
        assert!(matches!(
            WriteReq::decode(&bytes[..65]),
            Err(DecodeError::Truncated { .. })
        ));
        let res = WriteRes {
            sequence: 2,
            ar_uuid: AR,
            api: 0,
            slot: 1,
            subslot: 1,
            index: 0x0064,
            status: Status::from_u32(0xDF80_B600),
        };
        let bytes = res.encode();
        assert_eq!(bytes.len(), 64);
        assert_eq!(WriteRes::decode(&bytes).unwrap(), res);
    }
}
