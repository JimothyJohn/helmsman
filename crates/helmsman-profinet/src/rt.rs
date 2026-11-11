//! Cyclic data: what an RT class 1 frame carries after its `FrameID`, and
//! where each submodule's bytes sit in it.
//!
//! The frame's data part (`C_SDU`) holds, for every submodule, its data
//! followed by a provider status byte (IOPS), and for data going the
//! other way a consumer status byte (IOCS). After the data come a 16-bit
//! cycle counter, the data status and the transfer status (p-net
//! `pf_ppm.c` `pf_ppm_finish_buffer`). A receiver finds those four bytes
//! at the end of the frame and takes only a frame of exactly the agreed
//! length (`pf_cpm_driver_sw.c`), which is why the data part is at least
//! 40 bytes: the frame then never needs Ethernet padding.
//!
//! The controller decides the layout and tells the device in the Connect
//! request ([`crate::block::IocrReq`]); [`Layout`] is that decision, used
//! by both state machines.

use alloc::vec::Vec;

use crate::block::{
    DIRECTION_INPUT, DIRECTION_OUTPUT, DataDescription, ExpectedModule, ExpectedSubmodule, FrameOffset,
    IocrApi,
};
use crate::codec::{DecodeError, Reader, Writer};

/// A status byte saying the data beside it is good (p-net `pnet_api.h`
/// `PNET_IOXS_GOOD`), and one saying it is not.
pub const IOXS_GOOD: u8 = 0x80;
pub const IOXS_BAD: u8 = 0x00;

/// Data status bits (p-net `pnet_api.h` `PNET_DATA_STATUS_BIT_*`).
pub const STATUS_PRIMARY: u8 = 0x01;
pub const STATUS_DATA_VALID: u8 = 0x04;
/// The provider is running (set) or stopped (clear).
pub const STATUS_RUN: u8 = 0x10;
/// Set when the station has no problem to report.
pub const STATUS_NORMAL: u8 = 0x20;
/// What a healthy, running provider sends.
pub const STATUS_OK: u8 = STATUS_PRIMARY | STATUS_DATA_VALID | STATUS_RUN | STATUS_NORMAL;

/// The shortest and longest data part of an RT class 1 frame (p-net
/// `pf_cmdev.c` `pf_cmdev_check_iocr_param`).
pub const MIN_DATA: u16 = 40;
pub const MAX_DATA: u16 = 1440;

/// One record written to a submodule during startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub index: u16,
    pub data: Vec<u8>,
}

/// One submodule of the expected configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submodule {
    pub subslot: u16,
    pub ident: u32,
    /// Bytes of input data (device to controller); 0 for none.
    pub input: u16,
    /// Bytes of output data (controller to device); 0 for none.
    pub output: u16,
    /// Parameters the controller writes before `PrmEnd`.
    pub records: Vec<Record>,
}

/// One module of the expected configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub api: u32,
    pub slot: u16,
    pub ident: u32,
    pub submodules: Vec<Submodule>,
}

/// Where one submodule's bytes sit: in the two frames, and in the two
/// process images (every submodule's input data one after another, and
/// likewise the outputs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    pub api: u32,
    pub slot: u16,
    pub subslot: u16,
    pub input_len: u16,
    pub output_len: u16,
    /// Offset of its input data in the input image.
    pub input_image: usize,
    /// Offset of its output data in the output image.
    pub output_image: usize,
    /// In the input frame: its data, then its IOPS. A submodule without
    /// data in either direction has a place here too, of length 0: its
    /// IOPS says whether it is there at all.
    pub input_frame: Option<u16>,
    /// In the input frame: the device's IOCS for its output data.
    pub input_iocs: Option<u16>,
    /// In the output frame: its data, then its IOPS.
    pub output_frame: Option<u16>,
    /// In the output frame: the controller's IOCS for its input data.
    pub output_iocs: Option<u16>,
}

/// The layout of both frames and both images for a configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub places: Vec<Place>,
    /// Data part length of the input frame (at least [`MIN_DATA`]).
    pub input_frame_len: u16,
    pub output_frame_len: u16,
    pub input_image_len: usize,
    pub output_image_len: usize,
}

impl Layout {
    /// Lays the submodules out in the order given: data objects packed
    /// from offset 0, each followed by its one-byte IOPS, then the
    /// consumer statuses.
    ///
    /// # Errors
    /// A frame's data part would exceed [`MAX_DATA`].
    pub fn new(modules: &[Module]) -> Result<Self, DecodeError> {
        let too_long = DecodeError::Invalid("cyclic data longer than 1440 bytes");
        let mut places = Vec::new();
        let (mut in_pos, mut out_pos) = (0u32, 0u32);
        let (mut in_image, mut out_image) = (0usize, 0usize);
        let offset = |pos: u32| u16::try_from(pos).map_err(|_| too_long.clone());
        for m in modules {
            for s in &m.submodules {
                let mut p = Place {
                    api: m.api,
                    slot: m.slot,
                    subslot: s.subslot,
                    input_len: s.input,
                    output_len: s.output,
                    input_image: in_image,
                    output_image: out_image,
                    input_frame: None,
                    input_iocs: None,
                    output_frame: None,
                    output_iocs: None,
                };
                if s.input > 0 || s.output == 0 {
                    p.input_frame = Some(offset(in_pos)?);
                    in_pos += u32::from(s.input) + 1;
                    in_image += usize::from(s.input);
                }
                if s.output > 0 {
                    p.output_frame = Some(offset(out_pos)?);
                    out_pos += u32::from(s.output) + 1;
                    out_image += usize::from(s.output);
                }
                places.push(p);
            }
        }
        for p in &mut places {
            if p.output_frame.is_some() {
                p.input_iocs = Some(offset(in_pos)?);
                in_pos += 1;
            }
            if p.input_frame.is_some() {
                p.output_iocs = Some(offset(out_pos)?);
                out_pos += 1;
            }
        }
        let frame_len = |pos: u32| match u16::try_from(pos) {
            Ok(n) if n <= MAX_DATA => Ok(n.max(MIN_DATA)),
            _ => Err(too_long.clone()),
        };
        Ok(Self {
            places,
            input_frame_len: frame_len(in_pos)?,
            output_frame_len: frame_len(out_pos)?,
            input_image_len: in_image,
            output_image_len: out_image,
        })
    }

    fn apis(
        &self,
        data: impl Fn(&Place) -> Option<u16>,
        iocs: impl Fn(&Place) -> Option<u16>,
    ) -> Vec<IocrApi> {
        let mut apis: Vec<IocrApi> = Vec::new();
        for p in &self.places {
            let at = apis.iter().position(|a| a.api == p.api).unwrap_or_else(|| {
                apis.push(IocrApi {
                    api: p.api,
                    ..IocrApi::default()
                });
                apis.len() - 1
            });
            let entry = |offset| FrameOffset {
                slot: p.slot,
                subslot: p.subslot,
                offset,
            };
            if let Some(o) = data(p) {
                apis[at].data.push(entry(o));
            }
            if let Some(o) = iocs(p) {
                apis[at].iocs.push(entry(o));
            }
        }
        apis
    }

    /// The input frame's layout as an `IOCRBlockReq` says it.
    #[must_use]
    pub fn input_apis(&self) -> Vec<IocrApi> {
        self.apis(|p| p.input_frame, |p| p.input_iocs)
    }

    /// The output frame's layout as an `IOCRBlockReq` says it.
    #[must_use]
    pub fn output_apis(&self) -> Vec<IocrApi> {
        self.apis(|p| p.output_frame, |p| p.output_iocs)
    }
}

/// The configuration as the `ExpectedSubmoduleBlockReq` says it: every
/// status one byte; a submodule without data described as an input of
/// length 0 (p-net `pf_cmdev.c`
/// `pf_cmdev_calculate_exp_sub_data_descriptor_direction`: "A module with
/// no cyclic data is implemented an input module").
#[must_use]
pub fn expected(modules: &[Module]) -> Vec<ExpectedModule> {
    let describe = |direction, length| DataDescription {
        direction,
        length,
        iocs_length: 1,
        iops_length: 1,
    };
    modules
        .iter()
        .map(|m| ExpectedModule {
            api: m.api,
            slot: m.slot,
            ident: m.ident,
            properties: 0,
            submodules: m
                .submodules
                .iter()
                .map(|s| {
                    let (properties, data) = match (s.input > 0, s.output > 0) {
                        (true, true) => (
                            3,
                            alloc::vec![
                                describe(DIRECTION_INPUT, s.input),
                                describe(DIRECTION_OUTPUT, s.output)
                            ],
                        ),
                        (false, true) => (2, alloc::vec![describe(DIRECTION_OUTPUT, s.output)]),
                        (true, false) => (1, alloc::vec![describe(DIRECTION_INPUT, s.input)]),
                        (false, false) => (0, alloc::vec![describe(DIRECTION_INPUT, 0)]),
                    };
                    ExpectedSubmodule {
                        subslot: s.subslot,
                        ident: s.ident,
                        properties,
                        data,
                    }
                })
                .collect(),
        })
        .collect()
}

/// The modules an `ExpectedSubmoduleBlockReq` describes, as a device
/// reads them back (records are not part of it).
#[must_use]
pub fn modules_of(expected: &[ExpectedModule]) -> Vec<Module> {
    expected
        .iter()
        .map(|m| Module {
            api: m.api,
            slot: m.slot,
            ident: m.ident,
            submodules: m
                .submodules
                .iter()
                .map(|s| {
                    let len = |direction| {
                        s.data
                            .iter()
                            .find(|d| d.direction == direction)
                            .map_or(0, |d| d.length)
                    };
                    Submodule {
                        subslot: s.subslot,
                        ident: s.ident,
                        input: len(DIRECTION_INPUT),
                        output: len(DIRECTION_OUTPUT),
                        records: Vec::new(),
                    }
                })
                .collect(),
        })
        .collect()
}

/// A cyclic frame's payload, read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cyclic<'a> {
    pub data: &'a [u8],
    pub cycle: u16,
    pub data_status: u8,
    pub transfer_status: u8,
}

/// The payload after the `FrameID`: the data part, the cycle counter, the
/// data status and a transfer status of 0.
#[must_use]
pub fn encode(data: &[u8], cycle: u16, data_status: u8) -> Vec<u8> {
    let mut w = Writer::with_capacity(data.len() + 4);
    w.bytes(data).u16(cycle).u8(data_status).u8(0);
    w.into_inner()
}

/// Reads a cyclic payload whose data part is `data_len` long.
///
/// # Errors
/// Shorter than the data part and its four trailing bytes.
pub fn parse(payload: &[u8], data_len: u16) -> Result<Cyclic<'_>, DecodeError> {
    let mut r = Reader::new(payload);
    Ok(Cyclic {
        data: r.bytes(usize::from(data_len))?,
        cycle: r.u16()?,
        data_status: r.u8()?,
        transfer_status: r.u8()?,
    })
}

/// The cycle counter at `now_us`: time in units of 31.25 us, rounded
/// down to a multiple of the send interval (p-net `pf_ppm.c`
/// `pf_ppm_calculate_cyclecounter`).
#[must_use]
pub fn cycle_counter(now_us: u64, send_clock_factor: u16, reduction_ratio: u16) -> u16 {
    let interval = (u64::from(send_clock_factor) * u64::from(reduction_ratio)).max(1);
    let ticks = now_us.wrapping_mul(32) / 1000;
    // Truncation is the point: the counter is 16 bits and wraps.
    #[allow(clippy::cast_possible_truncation)]
    let counter = (ticks / interval * interval) as u16;
    counter
}

/// Whether a frame with counter `now` is newer than one with `prev`, as a
/// consumer decides it (p-net `pf_cpm.c` `pf_cpm_check_cycle`: ahead by
/// 1..=61440).
#[must_use]
pub const fn newer(prev: u16, now: u16) -> bool {
    let diff = now.wrapping_sub(prev);
    diff >= 1 && diff <= 61440
}

/// The send interval in microseconds: `send_clock_factor` x
/// `reduction_ratio` x 31.25 us.
#[must_use]
pub fn interval_us(send_clock_factor: u16, reduction_ratio: u16) -> u64 {
    u64::from(send_clock_factor) * u64::from(reduction_ratio) * 125 / 4
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(subslot: u16, input: u16, output: u16) -> Submodule {
        Submodule {
            subslot,
            ident: u32::from(subslot),
            input,
            output,
            records: Vec::new(),
        }
    }

    fn modules() -> Vec<Module> {
        alloc::vec![
            Module {
                api: 0,
                slot: 0,
                ident: 1,
                submodules: alloc::vec![sub(1, 0, 0)],
            },
            Module {
                api: 0,
                slot: 1,
                ident: 0x30,
                submodules: alloc::vec![sub(1, 4, 0)],
            },
            Module {
                api: 0,
                slot: 2,
                ident: 0x31,
                submodules: alloc::vec![sub(1, 2, 3)],
            },
        ]
    }

    #[test]
    fn data_then_status_then_the_other_directions_consumer_statuses() {
        let l = Layout::new(&modules()).unwrap();
        // Input frame: DAP IOPS @0; slot 1 data @1..5, IOPS @5; slot 2 data
        // @6..8, IOPS @8; IOCS for slot 2's outputs @9.
        let at: Vec<_> = l.places.iter().map(|p| (p.input_frame, p.input_iocs)).collect();
        assert_eq!(at, [(Some(0), None), (Some(1), None), (Some(6), Some(9))]);
        // Output frame: slot 2 data @0..3, IOPS @3; IOCS for the DAP @4,
        // slot 1 @5, slot 2's inputs @6.
        let at: Vec<_> = l.places.iter().map(|p| (p.output_frame, p.output_iocs)).collect();
        assert_eq!(at, [(None, Some(4)), (None, Some(5)), (Some(0), Some(6))]);
        assert_eq!((l.input_frame_len, l.output_frame_len), (MIN_DATA, MIN_DATA));
        assert_eq!((l.input_image_len, l.output_image_len), (6, 3));
        assert_eq!(l.places[2].input_image, 4);
        let api = &l.input_apis()[0];
        assert_eq!((api.data.len(), api.iocs.len()), (3, 1));
        let api = &l.output_apis()[0];
        assert_eq!((api.data.len(), api.iocs.len()), (1, 3));
    }

    #[test]
    fn the_expected_blocks_describe_each_direction_and_read_back() {
        let m = modules();
        let e = expected(&m);
        assert_eq!(e[0].submodules[0].properties, 0);
        assert_eq!(e[0].submodules[0].data.len(), 1);
        assert_eq!(e[2].submodules[0].properties, 3);
        assert_eq!(e[2].submodules[0].data[1].direction, DIRECTION_OUTPUT);
        assert_eq!(modules_of(&e), m);
    }

    #[test]
    fn a_configuration_too_big_for_a_frame_is_refused() {
        let big = alloc::vec![Module {
            api: 0,
            slot: 1,
            ident: 1,
            submodules: alloc::vec![sub(1, 1000, 0), sub(2, 440, 0)],
        }];
        assert!(Layout::new(&big).is_err());
        let fits = alloc::vec![Module {
            api: 0,
            slot: 1,
            ident: 1,
            submodules: alloc::vec![sub(1, 1438, 0)],
        }];
        assert_eq!(Layout::new(&fits).unwrap().input_frame_len, 1439);
    }

    #[test]
    fn a_cyclic_payload_reads_back() {
        let bytes = encode(&[7; 40], 0x1234, STATUS_OK);
        assert_eq!(&bytes[40..], &[0x12, 0x34, 0x35, 0]);
        let c = parse(&bytes, 40).unwrap();
        assert_eq!((c.data, c.cycle, c.data_status), (&[7u8; 40][..], 0x1234, 0x35));
        assert!(parse(&bytes[..43], 40).is_err());
    }

    #[test]
    fn the_cycle_counter_steps_by_the_send_interval() {
        // 1 ms base clock, a frame every 8 ms: 256 units of 31.25 us.
        assert_eq!(interval_us(32, 8), 8000);
        assert_eq!(cycle_counter(0, 32, 8), 0);
        assert_eq!(cycle_counter(7_999, 32, 8), 0);
        assert_eq!(cycle_counter(8_000, 32, 8), 256);
        assert_eq!(cycle_counter(16_000, 32, 8), 512);
        // 2.048 s is 65536 units: the counter wraps.
        assert_eq!(cycle_counter(2_048_000, 32, 8), 0);
        assert!(newer(0xFF00, 0x0000));
        assert!(newer(10, 11));
        assert!(!newer(11, 11));
        assert!(!newer(11, 10));
    }
}
