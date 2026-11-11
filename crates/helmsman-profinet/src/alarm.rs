//! Alarms: the acknowledged transport (RTA) a device reports events over,
//! and the two blocks it carries.
//!
//! After the `FrameID` ([`crate::eth::FRAME_ID_ALARM_HIGH`] or
//! [`crate::eth::FRAME_ID_ALARM_LOW`]) comes a fixed part: the receiver's
//! and the sender's alarm reference (exchanged in the Connect), a PDU
//! type and version, flags (window size 1, and whether an
//! acknowledgement is wanted), a send and an acknowledge sequence number
//! and the length of what follows (p-net `pf_block_writer.c`
//! `pf_put_alarm_fixed`, `pf_alarm.c`). A DATA PDU carries an
//! `AlarmNotification` (device to controller) or an `AlarmAck` (the
//! controller's answer, which the device acknowledges in turn).
//!
//! Sequence numbers start at 0xFFFF for the first DATA PDU, with 0xFFFE
//! as "nothing acknowledged yet", and then count 0, 1, .. 0x7FFF (p-net
//! `pf_alarm.c`: `send_seq_count = 0xffff`, `exp_seq_count_o = 0xfffe`).

use alloc::vec::Vec;

use crate::codec::{DecodeError, Reader, Writer};
use crate::rpc::Status;

pub const PDU_DATA: u8 = 1;
pub const PDU_NACK: u8 = 2;
pub const PDU_ACK: u8 = 3;
pub const PDU_ERR: u8 = 4;

/// Block types (p-net `pf_types.h`).
pub const NOTIFICATION_HIGH: u16 = 0x0001;
pub const NOTIFICATION_LOW: u16 = 0x0002;
pub const ACK_HIGH: u16 = 0x8001;
pub const ACK_LOW: u16 = 0x8002;

/// The first send sequence number, and the "none yet" acknowledge number.
pub const SEQ_FIRST: u16 = 0xFFFF;
pub const SEQ_NONE: u16 = 0xFFFE;

/// The sequence number after `seq`.
#[must_use]
pub const fn next_seq(seq: u16) -> u16 {
    seq.wrapping_add(1) & 0x7FFF
}

/// An RTA PDU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rta {
    /// The receiver's alarm reference.
    pub dst_ref: u16,
    /// The sender's.
    pub src_ref: u16,
    pub pdu_type: u8,
    /// Whether the sender wants a transport acknowledgement.
    pub tack: bool,
    pub send_seq: u16,
    pub ack_seq: u16,
    /// The variable part: a block for DATA, a status for ERR, nothing
    /// for ACK and NACK.
    pub var: Vec<u8>,
}

impl Rta {
    /// The payload after the `FrameID`.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(12 + self.var.len());
        w.u16(self.dst_ref).u16(self.src_ref);
        // Version 1 in the high nibble; window size 1 in the low nibble
        // of the flags, the acknowledge request in bit 4.
        w.u8(0x10 | (self.pdu_type & 0x0F)).u8(0x01 | (u8::from(self.tack) << 4));
        w.u16(self.send_seq).u16(self.ack_seq);
        w.u16(u16::try_from(self.var.len()).unwrap_or(u16::MAX)).bytes(&self.var);
        w.into_inner()
    }

    /// # Errors
    /// Truncated, or not version 1.
    pub fn parse(payload: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(payload);
        let dst_ref = r.u16()?;
        let src_ref = r.u16()?;
        let ty = r.u8()?;
        if ty >> 4 != 1 {
            return Err(DecodeError::Invalid("RTA version"));
        }
        let flags = r.u8()?;
        let send_seq = r.u16()?;
        let ack_seq = r.u16()?;
        let len = usize::from(r.u16()?);
        Ok(Self {
            dst_ref,
            src_ref,
            pdu_type: ty & 0x0F,
            tack: flags & 0x10 != 0,
            send_seq,
            ack_seq,
            var: r.bytes(len)?.into(),
        })
    }
}

/// An `AlarmNotification`: what happened and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    /// High priority (block 0x0001) or low (0x0002).
    pub high: bool,
    /// 1 diagnosis, 2 process, 3 pull, 4 plug, .. (p-net `pf_types.h`
    /// `PF_ALARM_TYPE_*`).
    pub alarm_type: u16,
    pub api: u32,
    pub slot: u16,
    pub subslot: u16,
    pub module_ident: u32,
    pub submodule_ident: u32,
    /// The alarm specifier: a sequence number in the low eleven bits and
    /// the diagnosis flags above it.
    pub specifier: u16,
    /// What follows: a user structure identifier and its data, or
    /// nothing.
    pub data: Vec<u8>,
}

impl Notification {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(32 + self.data.len());
        w.u16(if self.high {
            NOTIFICATION_HIGH
        } else {
            NOTIFICATION_LOW
        });
        w.u16(u16::try_from(2 + 20 + self.data.len()).unwrap_or(u16::MAX));
        w.u8(1).u8(0);
        w.u16(self.alarm_type).u32(self.api).u16(self.slot).u16(self.subslot);
        w.u32(self.module_ident).u32(self.submodule_ident).u16(self.specifier);
        w.bytes(&self.data);
        w.into_inner()
    }

    /// # Errors
    /// Malformed, or not a notification block.
    pub fn parse(var: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(var);
        let crate::block::Block { block_type, mut body } = crate::block::next(&mut r)?;
        let high = match block_type {
            NOTIFICATION_HIGH => true,
            NOTIFICATION_LOW => false,
            _ => return Err(DecodeError::Invalid("not an alarm notification")),
        };
        Ok(Self {
            high,
            alarm_type: body.u16()?,
            api: body.u32()?,
            slot: body.u16()?,
            subslot: body.u16()?,
            module_ident: body.u32()?,
            submodule_ident: body.u32()?,
            specifier: body.u16()?,
            data: body.rest().into(),
        })
    }

    /// The `AlarmAck` block that answers this notification.
    #[must_use]
    pub fn ack(&self, status: Status) -> Vec<u8> {
        let mut w = Writer::with_capacity(24);
        w.u16(if self.high { ACK_HIGH } else { ACK_LOW }).u16(2 + 16).u8(1).u8(0);
        w.u16(self.alarm_type).u32(self.api).u16(self.slot).u16(self.subslot);
        w.u16(self.specifier).u32(status.to_u32());
        w.into_inner()
    }
}

/// An `AlarmAck`, read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ack {
    pub high: bool,
    pub alarm_type: u16,
    pub api: u32,
    pub slot: u16,
    pub subslot: u16,
    pub specifier: u16,
    pub status: Status,
}

impl Ack {
    /// # Errors
    /// Malformed, or not an acknowledgement block.
    pub fn parse(var: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(var);
        let crate::block::Block { block_type, mut body } = crate::block::next(&mut r)?;
        let high = match block_type {
            ACK_HIGH => true,
            ACK_LOW => false,
            _ => return Err(DecodeError::Invalid("not an alarm acknowledgement")),
        };
        Ok(Self {
            high,
            alarm_type: body.u16()?,
            api: body.u32()?,
            slot: body.u16()?,
            subslot: body.u16()?,
            specifier: body.u16()?,
            status: Status::from_u32(body.u32()?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification() -> Notification {
        Notification {
            high: false,
            alarm_type: 1,
            api: 0,
            slot: 2,
            subslot: 1,
            module_ident: 0x31,
            submodule_ident: 0x131,
            specifier: 0x0801,
            data: alloc::vec![0x80, 0x00, 1, 2],
        }
    }

    #[test]
    fn a_data_pdu_is_the_bytes_on_the_wire() {
        let rta = Rta {
            dst_ref: 1,
            src_ref: 7,
            pdu_type: PDU_DATA,
            tack: true,
            send_seq: SEQ_FIRST,
            ack_seq: SEQ_NONE,
            var: notification().encode(),
        };
        let bytes = rta.encode();
        assert_eq!(&bytes[..12], &[0, 1, 0, 7, 0x11, 0x11, 0xFF, 0xFF, 0xFF, 0xFE, 0, 30]);
        // The block: low-priority notification, length 2 + 20 + 4.
        assert_eq!(&bytes[12..18], &[0, 2, 0, 26, 1, 0]);
        let back = Rta::parse(&bytes).unwrap();
        assert_eq!(back, rta);
        assert_eq!(Notification::parse(&back.var).unwrap(), notification());
    }

    #[test]
    fn an_ack_pdu_has_no_variable_part_and_padding_is_ignored() {
        let ack = Rta {
            dst_ref: 7,
            src_ref: 1,
            pdu_type: PDU_ACK,
            tack: false,
            send_seq: SEQ_NONE,
            ack_seq: SEQ_FIRST,
            var: Vec::new(),
        };
        let mut bytes = ack.encode();
        assert_eq!(&bytes[4..], &[0x13, 0x01, 0xFF, 0xFE, 0xFF, 0xFF, 0, 0]);
        bytes.extend_from_slice(&[0; 30]);
        assert_eq!(Rta::parse(&bytes).unwrap(), ack);
    }

    #[test]
    fn an_alarm_ack_names_the_alarm_and_carries_a_status() {
        let n = notification();
        let block = n.ack(Status::OK);
        assert_eq!(&block[..6], &[0x80, 0x02, 0, 18, 1, 0]);
        let ack = Ack::parse(&block).unwrap();
        assert_eq!((ack.alarm_type, ack.slot, ack.subslot, ack.specifier), (1, 2, 1, 0x0801));
        assert!(ack.status.is_ok());
        assert!(Ack::parse(&n.encode()).is_err());
        assert!(Notification::parse(&block).is_err());
    }

    #[test]
    fn sequence_numbers_wrap_into_fifteen_bits() {
        assert_eq!(next_seq(SEQ_FIRST), 0);
        assert_eq!(next_seq(0), 1);
        assert_eq!(next_seq(0x7FFF), 0);
    }

    #[test]
    fn other_versions_and_short_pdus_are_refused() {
        let mut bytes = Rta {
            dst_ref: 1,
            src_ref: 2,
            pdu_type: PDU_ACK,
            tack: false,
            send_seq: 0,
            ack_seq: 0,
            var: Vec::new(),
        }
        .encode();
        bytes[4] = 0x23;
        assert!(Rta::parse(&bytes).is_err());
        assert!(Rta::parse(&bytes[..11]).is_err());
    }
}
