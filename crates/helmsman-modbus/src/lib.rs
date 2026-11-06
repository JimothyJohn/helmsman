//! The bytes of Modbus, and nothing that moves them: request PDUs
//! ([`pdu`]), reply checks ([`reply`]) and TCP's MBAP header ([`mbap`]).
//! `no_std + alloc`, no dependencies. helmsman-io's TCP and serial clients
//! and the edge node's embassy-net client (crates/helmsman-node) share it,
//! so the box and a node put the same bytes on the wire and refuse the same
//! malformed replies.
//!
//! References (docs/SOURCES.md):
//! - MODBUS Application Protocol Specification V1.1b3 (Modbus Organization,
//!   2012-04-26), cited as "App §x p.y".
//! - MODBUS Messaging on TCP/IP Implementation Guide V1.0b (2006-10-24),
//!   cited as "TCP §x p.y".
//!
//! Addresses are the 0-based PDU addresses of App §4.4.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use core::fmt;

/// App §4.1 p.3: the PDU is at most 253 bytes.
pub const MAX_PDU: usize = 253;
/// TCP §3.1.3 p.5: transaction id, protocol id, length, unit id.
pub const MBAP_LEN: usize = 7;

pub const READ_COILS: u8 = 0x01;
pub const READ_DISCRETE_INPUTS: u8 = 0x02;
pub const READ_HOLDING_REGISTERS: u8 = 0x03;
pub const READ_INPUT_REGISTERS: u8 = 0x04;
pub const WRITE_SINGLE_COIL: u8 = 0x05;
pub const WRITE_SINGLE_REGISTER: u8 = 0x06;
pub const WRITE_MULTIPLE_COILS: u8 = 0x0F;
pub const WRITE_MULTIPLE_REGISTERS: u8 = 0x10;

/// App §6.19 p.41, §6.21 p.43: Encapsulated Interface Transport, the
/// function code Read Device Identification rides on.
pub const ENCAPSULATED_INTERFACE: u8 = 0x2B;
/// App §6.21 p.43: MEI type 14, Read Device Identification.
pub const MEI_READ_DEVICE_ID: u8 = 0x0E;
/// App §6.21 p.44: Read Device ID code 01, the basic objects as a stream.
pub const READ_DEVICE_ID_BASIC: u8 = 0x01;
/// App §6.21 p.43: the three mandatory (basic) objects.
pub const OBJECT_VENDOR_NAME: u8 = 0x00;
pub const OBJECT_PRODUCT_CODE: u8 = 0x01;
pub const OBJECT_MAJOR_MINOR_REVISION: u8 = 0x02;

/// App §6.1 p.11, §6.2 p.12: 1 to 2000 bits per read.
pub const MAX_READ_BITS: u16 = 2000;
/// App §6.3 p.15, §6.4 p.16: 1 to 125 registers per read.
pub const MAX_READ_REGS: u16 = 125;
/// App §6.11 p.29: 1 to 1968 coils per write.
pub const MAX_WRITE_COILS: u16 = 1968;
/// App §6.12 p.30: 1 to 123 registers per write.
pub const MAX_WRITE_REGS: u16 = 123;
/// App §6.5 p.17: the only two legal coil values.
pub const COIL_ON: u16 = 0xFF00;
pub const COIL_OFF: u16 = 0x0000;

/// App §7 p.47 exception codes.
pub const ILLEGAL_FUNCTION: u8 = 0x01;
pub const ILLEGAL_DATA_ADDRESS: u8 = 0x02;
pub const ILLEGAL_DATA_VALUE: u8 = 0x03;
pub const SERVER_DEVICE_FAILURE: u8 = 0x04;

/// What the codec can refuse: a request outside the spec's ranges or a
/// reply that is not what the request asks for, and a device's exception.
/// Transports add their own link errors around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A request the spec does not allow, or a reply that does not fit the
    /// request (wrong function, wrong length, bad header).
    Protocol(String),
    /// The device answered with an exception code (App §7).
    Exception(u8),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protocol(s) => write!(f, "protocol: {s}"),
            Self::Exception(c) => write!(f, "exception {c:#04x}"),
        }
    }
}

impl core::error::Error for Error {}

/// Request PDUs, App §6.
pub mod pdu {
    use alloc::format;
    use alloc::vec;
    use alloc::vec::Vec;

    use super::{
        Error, MAX_PDU, MAX_READ_BITS, MAX_READ_REGS, MAX_WRITE_COILS, MAX_WRITE_REGS, WRITE_MULTIPLE_COILS,
        WRITE_MULTIPLE_REGISTERS,
    };

    fn be(v: u16) -> [u8; 2] {
        v.to_be_bytes()
    }

    /// FC 0x01-0x04: function, start, quantity.
    ///
    /// # Errors
    /// Quantity outside the spec's range for the function.
    pub fn read(fc: u8, start: u16, qty: u16) -> Result<Vec<u8>, Error> {
        let max = if fc <= 0x02 { MAX_READ_BITS } else { MAX_READ_REGS };
        if qty == 0 || qty > max || start.checked_add(qty - 1).is_none() {
            return Err(Error::Protocol(format!(
                "read of {qty} from {start} is out of range"
            )));
        }
        let mut p = vec![fc];
        p.extend(be(start));
        p.extend(be(qty));
        Ok(p)
    }

    /// FC 0x05 / 0x06: function, address, value. The reply echoes it.
    #[must_use]
    pub fn write_single(fc: u8, addr: u16, value: u16) -> Vec<u8> {
        let mut p = vec![fc];
        p.extend(be(addr));
        p.extend(be(value));
        p
    }

    /// FC 0x0F: function, start, quantity, byte count, coils LSB-first,
    /// the last byte zero-padded (App §6.11 p.29).
    ///
    /// # Errors
    /// Zero or more than 1968 coils, or running past address 65535.
    pub fn write_coils(start: u16, values: &[bool]) -> Result<Vec<u8>, Error> {
        let qty = u16::try_from(values.len()).unwrap_or(u16::MAX);
        if qty == 0 || qty > MAX_WRITE_COILS || start.checked_add(qty - 1).is_none() {
            return Err(Error::Protocol(format!(
                "write of {qty} coils from {start} is out of range"
            )));
        }
        let bytes = usize::from(qty).div_ceil(8);
        let mut p = vec![WRITE_MULTIPLE_COILS];
        p.extend(be(start));
        p.extend(be(qty));
        p.push(u8::try_from(bytes).expect("at most 246"));
        let mut packed = vec![0u8; bytes];
        for (i, on) in values.iter().enumerate() {
            if *on {
                packed[i / 8] |= 1 << (i % 8);
            }
        }
        p.extend(packed);
        debug_assert!(p.len() <= MAX_PDU);
        Ok(p)
    }

    /// FC 0x10: function, start, quantity, byte count, values.
    ///
    /// # Errors
    /// Zero or more than 123 registers, or running past address 65535.
    pub fn write_registers(start: u16, values: &[u16]) -> Result<Vec<u8>, Error> {
        let qty = u16::try_from(values.len()).unwrap_or(u16::MAX);
        if qty == 0 || qty > MAX_WRITE_REGS || start.checked_add(qty - 1).is_none() {
            return Err(Error::Protocol(format!(
                "write of {qty} registers from {start} is out of range"
            )));
        }
        let mut p = vec![WRITE_MULTIPLE_REGISTERS];
        p.extend(be(start));
        p.extend(be(qty));
        p.push(u8::try_from(qty * 2).expect("at most 246"));
        for v in values {
            p.extend(be(*v));
        }
        debug_assert!(p.len() <= MAX_PDU);
        Ok(p)
    }

    /// FC 0x2B / MEI 0x0E: Read Device Identification, `code` (01-04)
    /// from object `object` (App §6.21 p.43).
    #[must_use]
    pub fn read_device_id(code: u8, object: u8) -> Vec<u8> {
        vec![
            super::ENCAPSULATED_INTERFACE,
            super::MEI_READ_DEVICE_ID,
            code,
            object,
        ]
    }
}

/// One Read Device Identification reply (App §6.21 pp.43-44): what the
/// server says it conforms to, whether more objects follow and from
/// which, and the objects in this reply, in wire order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceIdPage {
    /// 0x01-0x03 or 0x81-0x83; kept as sent, informational only.
    pub conformity: u8,
    /// `Some(next)`: More Follows was 0xFF and the next request starts at
    /// object `next`. `None`: More Follows was 0x00.
    pub next: Option<u8>,
    /// `(object id, value bytes)`; a value is at most 247 bytes.
    pub objects: alloc::vec::Vec<(u8, alloc::vec::Vec<u8>)>,
}

/// Reply checks, App §6 and §7: anything not exactly what the request asks
/// for is a protocol error, never a panic or a guess.
pub mod reply {
    use alloc::format;
    use alloc::vec::Vec;

    use super::{
        DeviceIdPage, ENCAPSULATED_INTERFACE, Error, MEI_READ_DEVICE_ID, WRITE_MULTIPLE_COILS,
        WRITE_MULTIPLE_REGISTERS,
    };

    /// FC 0x2B / MEI 0x0E, Read Device Identification (App §6.21
    /// pp.43-44): MEI type, Read Device ID code, conformity level, More
    /// Follows (0x00 or 0xFF), Next Object Id, Number of Objects, then that
    /// many objects of id, length, value. Every length is checked against
    /// what is left of the PDU, the count must match the objects present
    /// and nothing may follow the last one.
    ///
    /// # Errors
    /// An exception, another MEI type or Read Device ID code than asked
    /// for, a More Follows other than 0x00 or 0xFF, a count or a length
    /// that does not match the bytes, or bytes after the last object.
    pub fn device_id(code: u8, pdu: &[u8]) -> Result<DeviceIdPage, Error> {
        let b = body(ENCAPSULATED_INTERFACE, pdu)?;
        let &[mei, got_code, conformity, more, next, n, ref objects_bytes @ ..] = b else {
            return Err(Error::Protocol(format!(
                "device identification reply of {} bytes",
                pdu.len()
            )));
        };
        if mei != MEI_READ_DEVICE_ID {
            return Err(Error::Protocol(format!("MEI type {mei:#04x}, expected 0x0e")));
        }
        if got_code != code {
            return Err(Error::Protocol(format!(
                "Read Device ID code {got_code:#04x}, expected {code:#04x}"
            )));
        }
        let next = match more {
            0x00 => None,
            0xFF => Some(next),
            m => return Err(Error::Protocol(format!("More Follows {m:#04x}"))),
        };
        let mut rest: &[u8] = objects_bytes;
        let mut objects = Vec::with_capacity(usize::from(n));
        for k in 0..n {
            let &[id, len, ref tail @ ..] = rest else {
                return Err(Error::Protocol(format!("{n} objects announced, {k} present")));
            };
            let Some((value, after)) = tail.split_at_checked(usize::from(len)) else {
                return Err(Error::Protocol(format!(
                    "object {id:#04x} of {len} bytes, {} left",
                    tail.len()
                )));
            };
            objects.push((id, value.to_vec()));
            rest = after;
        }
        if !rest.is_empty() {
            return Err(Error::Protocol(format!(
                "{} bytes after the last of {n} objects",
                rest.len()
            )));
        }
        Ok(DeviceIdPage {
            conformity,
            next,
            objects,
        })
    }

    /// Strips the function code, turning an exception reply (function code +
    /// 0x80, one exception byte; App §7 p.47) into [`Error::Exception`].
    ///
    /// # Errors
    /// Empty reply, exception, or a reply to another function.
    pub fn body(fc: u8, pdu: &[u8]) -> Result<&[u8], Error> {
        match pdu {
            [] => Err(Error::Protocol("empty reply".into())),
            [f, code] if *f == fc | 0x80 => Err(Error::Exception(*code)),
            [f, ..] if *f == fc | 0x80 => Err(Error::Protocol("malformed exception reply".into())),
            [f, rest @ ..] if *f == fc => Ok(rest),
            [f, ..] => Err(Error::Protocol(format!(
                "reply function {f:#04x} to request {fc:#04x}"
            ))),
        }
    }

    /// FC 0x01/0x02: byte count, then bits LSB-first, last byte zero-padded
    /// (App §6.1 p.11).
    ///
    /// # Errors
    /// Byte count or length not matching the quantity asked for.
    pub fn bits(fc: u8, qty: u16, pdu: &[u8]) -> Result<Vec<bool>, Error> {
        let b = body(fc, pdu)?;
        let want = usize::from(qty).div_ceil(8);
        match b {
            [n, data @ ..] if usize::from(*n) == want && data.len() == want => Ok((0..usize::from(qty))
                .map(|i| data[i / 8] >> (i % 8) & 1 == 1)
                .collect()),
            _ => Err(Error::Protocol(format!("bit reply length for {qty} bits"))),
        }
    }

    /// FC 0x03/0x04: byte count (2 per register), then big-endian registers
    /// (App §6.3 p.15).
    ///
    /// # Errors
    /// Byte count or length not matching the quantity asked for.
    pub fn registers(fc: u8, qty: u16, pdu: &[u8]) -> Result<Vec<u16>, Error> {
        let b = body(fc, pdu)?;
        let want = usize::from(qty) * 2;
        match b {
            [n, data @ ..] if usize::from(*n) == want && data.len() == want => Ok(data
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes(*c))
                .collect()),
            _ => Err(Error::Protocol(format!(
                "register reply length for {qty} registers"
            ))),
        }
    }

    /// FC 0x05/0x06 echo the request; FC 0x0F and 0x10 echo start and
    /// quantity (App §6.5 p.17, §6.6 p.19, §6.11 p.29, §6.12 p.30).
    ///
    /// # Errors
    /// The echo differs from what was sent, or the request is too short to
    /// have been one of ours.
    pub fn echo(request: &[u8], pdu: &[u8]) -> Result<(), Error> {
        let Some(&fc) = request.first() else {
            return Err(Error::Protocol("empty request".into()));
        };
        let b = body(fc, pdu)?;
        let want = if fc == WRITE_MULTIPLE_REGISTERS || fc == WRITE_MULTIPLE_COILS {
            request.get(1..5)
        } else {
            request.get(1..)
        };
        match want {
            Some(w) if b == w => Ok(()),
            Some(_) => Err(Error::Protocol("write reply does not echo the request".into())),
            None => Err(Error::Protocol("write request too short".into())),
        }
    }
}

/// TCP's MBAP header (TCP §3.1.3 p.5): transaction id, protocol id 0,
/// length (unit id plus the PDU), unit id.
pub mod mbap {
    use alloc::format;
    use alloc::vec::Vec;

    use super::{Error, MAX_PDU, MBAP_LEN};

    /// One request frame: header plus `pdu`.
    ///
    /// # Errors
    /// A PDU that is empty or longer than the spec allows.
    pub fn frame(tid: u16, unit: u8, pdu: &[u8]) -> Result<Vec<u8>, Error> {
        if pdu.is_empty() || pdu.len() > MAX_PDU {
            return Err(Error::Protocol(format!("request PDU of {} bytes", pdu.len())));
        }
        let len = u16::try_from(pdu.len() + 1).expect("at most 254");
        let mut f = Vec::with_capacity(MBAP_LEN + pdu.len());
        f.extend(tid.to_be_bytes());
        f.extend(0u16.to_be_bytes());
        f.extend(len.to_be_bytes());
        f.push(unit);
        f.extend(pdu);
        Ok(f)
    }

    /// Checks a reply's header against the request's `tid` and `unit` and
    /// returns how many body bytes (the PDU) follow it: 1 to 253.
    ///
    /// # Errors
    /// Another protocol, another transaction, a length outside the spec, or
    /// another unit: a late or foreign reply is never taken for ours.
    pub fn check_head(head: &[u8; MBAP_LEN], tid: u16, unit: u8) -> Result<usize, Error> {
        let got_tid = u16::from_be_bytes([head[0], head[1]]);
        let proto = u16::from_be_bytes([head[2], head[3]]);
        let len = usize::from(u16::from_be_bytes([head[4], head[5]]));
        if proto != 0 {
            return Err(Error::Protocol(format!("protocol id {proto}")));
        }
        if got_tid != tid {
            return Err(Error::Protocol(format!("transaction {got_tid}, expected {tid}")));
        }
        // Length counts the unit id plus a PDU of 1..=253 bytes.
        if !(2..=MAX_PDU + 1).contains(&len) {
            return Err(Error::Protocol(format!("length field {len}")));
        }
        if head[6] != unit {
            return Err(Error::Protocol(format!("unit {}, expected {unit}", head[6])));
        }
        Ok(len - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_requests_are_bounded_by_the_spec() {
        assert_eq!(pdu::read(READ_COILS, 0, 1).unwrap(), [0x01, 0, 0, 0, 1]);
        assert_eq!(
            pdu::read(READ_HOLDING_REGISTERS, 0x1234, 125).unwrap(),
            [0x03, 0x12, 0x34, 0, 125]
        );
        assert!(pdu::read(READ_COILS, 0, 0).is_err());
        assert!(pdu::read(READ_COILS, 0, 2001).is_err());
        assert!(pdu::read(READ_COILS, 0, 2000).is_ok());
        assert!(pdu::read(READ_INPUT_REGISTERS, 0, 126).is_err());
        assert!(
            pdu::read(READ_INPUT_REGISTERS, 65535, 2).is_err(),
            "runs past 65535"
        );
        assert!(pdu::read(READ_INPUT_REGISTERS, 65535, 1).is_ok());
    }

    #[test]
    fn coil_writes_pack_lsb_first_and_are_bounded() {
        let p = pdu::write_coils(16, &[true, false, true, true, false, false, false, false, true]).unwrap();
        assert_eq!(p, [0x0F, 0, 16, 0, 9, 2, 0b0000_1101, 0b0000_0001]);
        assert!(pdu::write_coils(0, &[]).is_err());
        assert!(pdu::write_coils(0, &[false; 1969]).is_err());
        assert!(pdu::write_coils(0, &[false; 1968]).is_ok());
        assert!(pdu::write_coils(65535, &[true, true]).is_err());
        assert_eq!(
            pdu::write_single(WRITE_SINGLE_COIL, 3, COIL_ON),
            [0x05, 0, 3, 0xFF, 0x00]
        );
    }

    #[test]
    fn register_writes_are_bounded() {
        assert_eq!(
            pdu::write_registers(1, &[0xABCD]).unwrap(),
            [0x10, 0, 1, 0, 1, 2, 0xAB, 0xCD]
        );
        assert!(pdu::write_registers(0, &[]).is_err());
        assert!(pdu::write_registers(0, &[0; 124]).is_err());
        assert!(pdu::write_registers(0, &[0; 123]).is_ok());
    }

    #[test]
    fn replies_are_checked_byte_for_byte() {
        assert_eq!(
            reply::bits(READ_COILS, 10, &[0x01, 2, 0b1010_0101, 0b10])
                .unwrap()
                .len(),
            10
        );
        assert_eq!(
            reply::bits(READ_COILS, 3, &[0x01, 1, 0b101]).unwrap(),
            [true, false, true]
        );
        assert!(
            reply::bits(READ_COILS, 3, &[0x01, 2, 0b101, 0]).is_err(),
            "byte count off"
        );
        assert!(reply::bits(READ_COILS, 3, &[0x01, 1]).is_err(), "short");
        assert_eq!(
            reply::registers(READ_HOLDING_REGISTERS, 2, &[0x03, 4, 1, 2, 3, 4]).unwrap(),
            [0x0102, 0x0304]
        );
        assert!(reply::registers(READ_HOLDING_REGISTERS, 2, &[0x03, 4, 1, 2, 3]).is_err());
        assert_eq!(
            reply::body(READ_COILS, &[0x81, ILLEGAL_DATA_ADDRESS]),
            Err(Error::Exception(2))
        );
        assert!(reply::body(READ_COILS, &[0x81]).is_err(), "malformed exception");
        assert!(
            reply::body(READ_COILS, &[0x02, 1, 0]).is_err(),
            "another function"
        );
        assert!(reply::body(READ_COILS, &[]).is_err());
    }

    #[test]
    fn echoes_match_the_request_shape() {
        let w = pdu::write_single(WRITE_SINGLE_COIL, 3, COIL_ON);
        assert!(reply::echo(&w, &w).is_ok());
        assert!(reply::echo(&w, &[0x05, 0, 3, 0, 0]).is_err());
        let m = pdu::write_registers(5, &[1, 2]).unwrap();
        assert!(reply::echo(&m, &[0x10, 0, 5, 0, 2]).is_ok());
        assert!(reply::echo(&m, &[0x10, 0, 5, 0, 3]).is_err());
        let c = pdu::write_coils(5, &[true, true]).unwrap();
        assert!(reply::echo(&c, &[0x0F, 0, 5, 0, 2]).is_ok());
        assert!(reply::echo(&[], &[0x05]).is_err(), "empty request");
        assert!(
            reply::echo(&[0x10, 0], &[0x10, 0, 5, 0, 2]).is_err(),
            "short request"
        );
    }

    #[test]
    fn mbap_frames_and_checks_heads() {
        let f = mbap::frame(0x0102, 7, &[0x01, 0, 0, 0, 8]).unwrap();
        assert_eq!(f, [1, 2, 0, 0, 0, 6, 7, 0x01, 0, 0, 0, 8]);
        assert!(mbap::frame(1, 1, &[]).is_err());
        assert!(mbap::frame(1, 1, &[0; 254]).is_err());
        assert!(mbap::frame(1, 1, &[0; 253]).is_ok());
        assert_eq!(mbap::check_head(&[1, 2, 0, 0, 0, 3, 7], 0x0102, 7), Ok(2));
        assert!(
            mbap::check_head(&[1, 2, 0, 1, 0, 3, 7], 0x0102, 7).is_err(),
            "protocol id"
        );
        assert!(
            mbap::check_head(&[1, 3, 0, 0, 0, 3, 7], 0x0102, 7).is_err(),
            "transaction"
        );
        assert!(
            mbap::check_head(&[1, 2, 0, 0, 0, 1, 7], 0x0102, 7).is_err(),
            "length 1"
        );
        assert!(
            mbap::check_head(&[1, 2, 0, 0, 0, 255, 7], 0x0102, 7).is_err(),
            "length 255"
        );
        assert!(
            mbap::check_head(&[1, 2, 0, 0, 0, 3, 8], 0x0102, 7).is_err(),
            "unit"
        );
    }

    /// The spec's own example (App §6.21 p.45): basic identification in one
    /// response, lengths in hex.
    #[test]
    fn device_identification_reads_the_spec_example() {
        assert_eq!(
            pdu::read_device_id(READ_DEVICE_ID_BASIC, 0),
            [0x2B, 0x0E, 0x01, 0x00]
        );
        let mut r = vec![0x2B, 0x0E, 0x01, 0x01, 0x00, 0x00, 0x03];
        for (id, v) in [
            (0u8, &b"Company identification"[..]),
            (1, b"Product code XX"),
            (2, b"V2.11"),
        ] {
            r.push(id);
            r.push(u8::try_from(v.len()).unwrap());
            r.extend(v);
        }
        let page = reply::device_id(READ_DEVICE_ID_BASIC, &r).unwrap();
        assert_eq!(page.conformity, 0x01);
        assert_eq!(page.next, None);
        assert_eq!(page.objects.len(), 3);
        assert_eq!(page.objects[2], (2, b"V2.11".to_vec()));
        // More Follows 0xFF names the next object.
        let mut more = r.clone();
        more[4] = 0xFF;
        more[5] = 0x02;
        assert_eq!(
            reply::device_id(READ_DEVICE_ID_BASIC, &more).unwrap().next,
            Some(2)
        );
        let bad = |f: &dyn Fn(&mut Vec<u8>)| {
            let mut x = r.clone();
            f(&mut x);
            reply::device_id(READ_DEVICE_ID_BASIC, &x)
        };
        assert!(bad(&|x| x[1] = 0x0D).is_err(), "MEI type 13");
        assert!(bad(&|x| x[2] = 0x02).is_err(), "another code");
        assert!(bad(&|x| x[4] = 0x01).is_err(), "More Follows 1");
        assert!(bad(&|x| x[6] = 4).is_err(), "count says 4");
        assert!(bad(&|x| x[6] = 2).is_err(), "count says 2: trailing bytes");
        assert!(bad(&|x| x.truncate(x.len() - 1)).is_err(), "truncated value");
        assert!(bad(&|x| x[8] = 0xFF).is_err(), "length past the end");
        assert!(bad(&|x| x.push(0)).is_err(), "trailing byte");
        assert!(bad(&|x| x.truncate(6)).is_err(), "no count");
        assert_eq!(
            reply::device_id(READ_DEVICE_ID_BASIC, &[0xAB, 0x01]),
            Err(Error::Exception(ILLEGAL_FUNCTION))
        );
        assert!(reply::device_id(READ_DEVICE_ID_BASIC, &[0x03, 2, 0, 0]).is_err());
    }

    #[test]
    fn garbage_never_panics() {
        for fc in 0..=255u8 {
            for n in 0..40u8 {
                let junk: Vec<u8> = (0..n).map(|i| i.wrapping_mul(37).wrapping_add(fc)).collect();
                let _ = reply::body(fc, &junk);
                let _ = reply::bits(fc, u16::from(n) * 7, &junk);
                let _ = reply::registers(fc, u16::from(n), &junk);
                let _ = reply::echo(&junk, &junk);
            }
        }
    }
}
