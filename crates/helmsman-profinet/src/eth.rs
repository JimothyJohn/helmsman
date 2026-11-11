//! The Ethernet frame PROFINET's real-time traffic rides in: `EtherType`
//! 0x8892, optionally behind an IEEE 802.1Q tag, then a 16-bit `FrameID`
//! that says what the rest is (cyclic data, an alarm, DCP).
//!
//! Layout as p-net builds it (`pf_ppm.c` `pf_ppm_init_buf`): destination,
//! source, TPID 0x8100, TCI (priority in the three high bits, VLAN id in
//! the low twelve), 0x8892, `FrameID`. A receiver takes the frame with or
//! without the tag (a switch may strip it).

use alloc::vec::Vec;

use crate::codec::{DecodeError, Reader, Writer};

/// A MAC address.
pub type Mac = [u8; 6];

/// PROFINET's `EtherType`.
pub const ETHERTYPE_PROFINET: u16 = 0x8892;
/// The IEEE 802.1Q tag protocol identifier.
pub const ETHERTYPE_VLAN: u16 = 0x8100;
/// The shortest Ethernet frame without its FCS; shorter ones are padded
/// with zeros here, so a transport that does not pad sends a legal frame.
pub const MIN_FRAME: usize = 60;

/// Cyclic data, RT class 1, unicast: 0xC000..=0xF7FF (p-net `pf_cmdev.c`
/// `pf_cmdev_check_iocr_param`, the range it accepts for an input CR and
/// allocates from for an output CR).
pub const FRAME_ID_RTC1_FIRST: u16 = 0xC000;
pub const FRAME_ID_RTC1_LAST: u16 = 0xF7FF;
/// Alarms, high and low priority (p-net `pf_alarm.c`).
pub const FRAME_ID_ALARM_HIGH: u16 = 0xFC01;
pub const FRAME_ID_ALARM_LOW: u16 = 0xFE01;
/// DCP (p-net `pf_dcp.c`): Hello, Get and Set, Identify request and
/// response.
pub const FRAME_ID_DCP_HELLO: u16 = 0xFEFC;
pub const FRAME_ID_DCP_GET_SET: u16 = 0xFEFD;
pub const FRAME_ID_DCP_IDENTIFY_REQ: u16 = 0xFEFE;
pub const FRAME_ID_DCP_IDENTIFY_RES: u16 = 0xFEFF;

/// An 802.1Q tag's two fields PROFINET uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vlan {
    /// Priority code point, 0..=7. Cyclic data and high alarms go out at
    /// 6, low alarms at 5.
    pub priority: u8,
    /// VLAN id, 0..=4095; 0 is "priority tag only".
    pub id: u16,
}

impl Vlan {
    /// The tag cyclic data carries: priority 6, no VLAN.
    pub const RT: Self = Self { priority: 6, id: 0 };

    /// The tag control information word.
    #[must_use]
    pub const fn tci(self) -> u16 {
        ((self.priority as u16 & 7) << 13) | (self.id & 0x0FFF)
    }

    #[must_use]
    pub const fn from_tci(tci: u16) -> Self {
        Self {
            priority: (tci >> 13) as u8,
            id: tci & 0x0FFF,
        }
    }
}

/// A received PROFINET frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame<'a> {
    pub dst: Mac,
    pub src: Mac,
    pub vlan: Option<Vlan>,
    pub frame_id: u16,
    /// Everything after the `FrameID`, the frame's padding included.
    pub payload: &'a [u8],
}

/// Builds a frame, padded to [`MIN_FRAME`].
#[must_use]
pub fn encode(dst: Mac, src: Mac, vlan: Option<Vlan>, frame_id: u16, payload: &[u8]) -> Vec<u8> {
    let mut w = Writer::with_capacity(MIN_FRAME.max(20 + payload.len()));
    w.bytes(&dst).bytes(&src);
    if let Some(v) = vlan {
        w.u16(ETHERTYPE_VLAN).u16(v.tci());
    }
    w.u16(ETHERTYPE_PROFINET).u16(frame_id).bytes(payload);
    if w.len() < MIN_FRAME {
        w.zeros(MIN_FRAME - w.len());
    }
    w.into_inner()
}

/// Reads a frame's addresses, tag and `FrameID`.
///
/// # Errors
/// Truncated, or not `EtherType` 0x8892 (tagged or not).
pub fn parse(bytes: &[u8]) -> Result<Frame<'_>, DecodeError> {
    let mut r = Reader::new(bytes);
    let dst = r.array()?;
    let src = r.array()?;
    let mut ethertype = r.u16()?;
    let mut vlan = None;
    if ethertype == ETHERTYPE_VLAN {
        vlan = Some(Vlan::from_tci(r.u16()?));
        ethertype = r.u16()?;
    }
    if ethertype != ETHERTYPE_PROFINET {
        return Err(DecodeError::Invalid("not a PROFINET frame"));
    }
    let frame_id = r.u16()?;
    Ok(Frame {
        dst,
        src,
        vlan,
        frame_id,
        payload: r.rest(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Mac = [2, 0, 0, 0, 0, 1];
    const B: Mac = [2, 0, 0, 0, 0, 2];

    #[test]
    fn a_tagged_frame_reads_back() {
        let bytes = encode(A, B, Some(Vlan::RT), 0xC000, &[1, 2, 3]);
        assert_eq!(bytes.len(), MIN_FRAME);
        assert_eq!(&bytes[12..18], &[0x81, 0x00, 0xC0, 0x00, 0x88, 0x92]);
        let f = parse(&bytes).unwrap();
        assert_eq!((f.dst, f.src, f.vlan, f.frame_id), (A, B, Some(Vlan::RT), 0xC000));
        assert_eq!(&f.payload[..3], &[1, 2, 3]);
    }

    #[test]
    fn an_untagged_frame_reads_back() {
        let bytes = encode(A, B, None, FRAME_ID_DCP_IDENTIFY_REQ, &[0; 80]);
        assert_eq!(bytes.len(), 14 + 2 + 80);
        let f = parse(&bytes).unwrap();
        assert_eq!((f.vlan, f.frame_id, f.payload.len()), (None, 0xFEFE, 80));
    }

    #[test]
    fn other_ethertypes_and_short_frames_are_refused() {
        let mut ip = encode(A, B, None, 0, &[]);
        ip[12..14].copy_from_slice(&[0x08, 0x00]);
        assert_eq!(parse(&ip), Err(DecodeError::Invalid("not a PROFINET frame")));
        assert!(matches!(parse(&ip[..13]), Err(DecodeError::Truncated { .. })));
        assert!(matches!(parse(&[]), Err(DecodeError::Truncated { .. })));
    }
}
