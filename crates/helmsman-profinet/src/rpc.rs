//! The DCE/RPC envelope of PROFINET's context management: connectionless
//! RPC over UDP, an 80-byte header and an NDR argument header in front of
//! the blocks ([`crate::block`]).
//!
//! The header is the connectionless PDU header of DCE 1.1: RPC (Open
//! Group C706, ch. 12 "RPC PDU Encodings"), field for field as p-net reads
//! it (`pf_block_reader.c` `pf_get_dce_rpc_header`). Its integers and the
//! NDR header's follow the sender's data representation (the first `drep`
//! byte's high nibble: 0 big-endian, 1 little-endian); the blocks after
//! them are big-endian whatever it says (p-net `pf_cmrpc.c`: "From now on
//! all is big-endian").

use alloc::vec::Vec;
use core::fmt;

use crate::codec::{DecodeError, Order, Reader, Writer};

/// The UDP port a device's and a controller's context manager listens on
/// (p-net `pf_types.h` `PF_RPC_SERVER_PORT`): a controller sends Connect
/// here, and a device sends its "application ready" to this port on the
/// controller (`pf_cmrpc.c` `pf_cmrpc_rm_ccontrol_req`).
pub const PORT: u16 = 0x8894;

/// The header's length.
pub const HEADER_LEN: usize = 80;
/// The largest connectionless PDU a peer must take, header included, and
/// so the largest body that needs no fragmenting (p-net `pf_cmrpc.c`
/// `PF_CMRPC_MUST_RECV_FRAG_SIZE`, citing DCE 1.1: RPC p. 578).
pub const MAX_PDU: usize = 1464;
pub const MAX_BODY: usize = MAX_PDU - HEADER_LEN;

/// Operation numbers of the IO device interface (p-net `pf_types.h`
/// `PF_RPC_DEV_OPNUM_*`). "Application ready" arrives at the controller
/// interface as [`OP_CONTROL`] too.
pub const OP_CONNECT: u16 = 0;
pub const OP_RELEASE: u16 = 1;
pub const OP_READ: u16 = 2;
pub const OP_WRITE: u16 = 3;
pub const OP_CONTROL: u16 = 4;
pub const OP_READ_IMPLICIT: u16 = 5;

/// Packet types (C706 ch. 12; p-net `PF_RPC_PT_*`).
pub const PT_REQUEST: u8 = 0;
pub const PT_PING: u8 = 1;
pub const PT_RESPONSE: u8 = 2;
pub const PT_FAULT: u8 = 3;
pub const PT_WORKING: u8 = 4;
pub const PT_NOCALL: u8 = 5;
pub const PT_REJECT: u8 = 6;
pub const PT_ACK: u8 = 7;
pub const PT_FACK: u8 = 9;

/// Header flags (first flags byte).
pub const FLAG_LAST_FRAGMENT: u8 = 0x02;
pub const FLAG_FRAGMENT: u8 = 0x04;
pub const FLAG_NO_FACK: u8 = 0x08;
pub const FLAG_IDEMPOTENT: u8 = 0x20;

/// A UUID, held in its text order (`dea00001-6c97-...` is bytes `DE A0 00
/// 01 6C 97 ...`). On the wire inside a block that is the byte order; in
/// the RPC header the first three fields follow the data representation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Uuid(pub [u8; 16]);

impl Uuid {
    pub const NIL: Self = Self([0; 16]);

    /// From the four fields of the usual notation.
    #[must_use]
    pub const fn new(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Self {
        let a = d1.to_be_bytes();
        let b = d2.to_be_bytes();
        let c = d3.to_be_bytes();
        Self([
            a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5], d4[6],
            d4[7],
        ])
    }

    /// As it travels in the RPC header.
    pub fn put(&self, w: &mut Writer, order: Order) {
        let b = &self.0;
        w.u32_in(order, u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
        w.u16_in(order, u16::from_be_bytes([b[4], b[5]]));
        w.u16_in(order, u16::from_be_bytes([b[6], b[7]]));
        w.bytes(&b[8..]);
    }

    /// # Errors
    /// `Truncated`.
    pub fn get(r: &mut Reader<'_>, order: Order) -> Result<Self, DecodeError> {
        let d1 = r.u32_in(order)?;
        let d2 = r.u16_in(order)?;
        let d3 = r.u16_in(order)?;
        Ok(Self::new(d1, d2, d3, r.array()?))
    }
}

impl fmt::Debug for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, b) in self.0.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

const TAIL: [u8; 8] = [0x82, 0x71, 0x00, 0xA0, 0x24, 0x42, 0xDF, 0x7D];
/// The IO device interface: what a controller calls (p-net `pf_cmrpc.c`
/// `uuid_io_device_interface`; Wireshark `uuid_pn_io_device`).
pub const IF_DEVICE: Uuid = Uuid::new(0xDEA0_0001, 0x6C97, 0x11D1, TAIL);
/// The IO controller interface: what a device calls for "application
/// ready" (p-net `pf_cmrpc_rm_ccontrol_req`; Wireshark
/// `uuid_pn_io_controller`).
pub const IF_CONTROLLER: Uuid = Uuid::new(0xDEA0_0002, 0x6C97, 0x11D1, TAIL);

/// The object UUID of a station: `dea00000-6c97-11d1-8271-` then its
/// instance, device id and vendor id, two bytes each (p-net
/// `pf_cmrpc_epm.c`, the lookup response's object UUID: "Interface
/// index", "Device ID", "Vendor ID"). A device checks the first ten bytes
/// of a controller's (`pf_cmdev.c`
/// `pf_cmdev_check_cm_initiator_object_uuid`).
#[must_use]
pub const fn object_uuid(instance: u16, device_id: u16, vendor_id: u16) -> Uuid {
    let i = instance.to_be_bytes();
    let d = device_id.to_be_bytes();
    let v = vendor_id.to_be_bytes();
    Uuid::new(
        0xDEA0_0000,
        0x6C97,
        0x11D1,
        [0x82, 0x71, i[0], i[1], d[0], d[1], v[0], v[1]],
    )
}

/// Whether `uuid` has the PROFINET object prefix.
#[must_use]
pub fn is_object_uuid(uuid: &Uuid) -> bool {
    uuid.0[..10] == object_uuid(0, 0, 0).0[..10]
}

/// The connectionless header, less the fields this crate always writes
/// the same way (version 4, ASCII and IEEE, no authentication, hints
/// 0xFFFF).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub packet_type: u8,
    pub flags: u8,
    /// The byte order of the header's and the NDR header's integers.
    pub order: Order,
    pub object: Uuid,
    pub interface: Uuid,
    /// One per client activity; a retransmission repeats it with the same
    /// sequence number.
    pub activity: Uuid,
    /// Zero in a client's first request; a client then echoes what the
    /// server's response said.
    pub server_boot: u32,
    pub sequence: u32,
    pub opnum: u16,
    pub fragment: u16,
}

impl Header {
    /// The header and `body` as one datagram.
    ///
    /// # Errors
    /// A body longer than [`MAX_BODY`]: this crate does not fragment.
    pub fn encode(&self, body: &[u8]) -> Result<Vec<u8>, DecodeError> {
        if body.len() > MAX_BODY {
            return Err(DecodeError::Invalid("RPC body needs fragmenting"));
        }
        let o = self.order;
        let mut w = Writer::with_capacity(HEADER_LEN + body.len());
        w.u8(4).u8(self.packet_type).u8(self.flags).u8(0);
        w.u8(if o == Order::Little { 0x10 } else { 0x00 }).u8(0).u8(0);
        w.u8(0);
        self.object.put(&mut w, o);
        self.interface.put(&mut w, o);
        self.activity.put(&mut w, o);
        w.u32_in(o, self.server_boot)
            .u32_in(o, 1)
            .u32_in(o, self.sequence);
        w.u16_in(o, self.opnum).u16_in(o, 0xFFFF).u16_in(o, 0xFFFF);
        // Checked against MAX_BODY above.
        w.u16_in(o, u16::try_from(body.len()).unwrap_or(u16::MAX));
        w.u16_in(o, self.fragment).u8(0).u8(0);
        w.bytes(body);
        Ok(w.into_inner())
    }

    /// Splits a datagram into header and body.
    ///
    /// # Errors
    /// Truncated, not version 4, or a body length past the datagram.
    pub fn decode(datagram: &[u8]) -> Result<(Self, &[u8]), DecodeError> {
        let mut r = Reader::new(datagram);
        if r.u8()? != 4 {
            return Err(DecodeError::Invalid("not connectionless RPC version 4"));
        }
        // Only the five low bits are the type (p-net `pf_get_dce_rpc_header`).
        let packet_type = r.u8()? & 0x1F;
        let flags = r.u8()?;
        r.u8()?;
        let order = if r.u8()? >> 4 == 0 {
            Order::Big
        } else {
            Order::Little
        };
        r.bytes(3)?;
        let object = Uuid::get(&mut r, order)?;
        let interface = Uuid::get(&mut r, order)?;
        let activity = Uuid::get(&mut r, order)?;
        let server_boot = r.u32_in(order)?;
        r.u32_in(order)?;
        let sequence = r.u32_in(order)?;
        let opnum = r.u16_in(order)?;
        r.bytes(4)?;
        let body_len = usize::from(r.u16_in(order)?);
        let fragment = r.u16_in(order)?;
        r.bytes(2)?;
        let body = r.bytes(body_len)?;
        Ok((
            Self {
                packet_type,
                flags,
                order,
                object,
                interface,
                activity,
                server_boot,
                sequence,
                opnum,
                fragment,
            },
            body,
        ))
    }

    /// Whether this PDU is one piece of a fragmented call, which this
    /// crate does not reassemble.
    #[must_use]
    pub const fn fragmented(&self) -> bool {
        self.flags & FLAG_FRAGMENT != 0
    }
}

/// A PNIO status: four bytes, all zero for success.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Status {
    pub code: u8,
    pub decode: u8,
    pub code1: u8,
    pub code2: u8,
}

impl Status {
    pub const OK: Self = Self {
        code: 0,
        decode: 0,
        code1: 0,
        code2: 0,
    };

    #[must_use]
    pub const fn is_ok(self) -> bool {
        self.code == 0 && self.decode == 0 && self.code1 == 0 && self.code2 == 0
    }

    /// As the integer the NDR header carries (p-net `pf_put_pnet_status`).
    #[must_use]
    pub const fn to_u32(self) -> u32 {
        u32::from_be_bytes([self.code, self.decode, self.code1, self.code2])
    }

    #[must_use]
    pub const fn from_u32(v: u32) -> Self {
        let b = v.to_be_bytes();
        Self {
            code: b[0],
            decode: b[1],
            code1: b[2],
            code2: b[3],
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PNIO status {:02x} {:02x} {:02x} {:02x}",
            self.code, self.decode, self.code1, self.code2
        )
    }
}

/// A request's body: the NDR header (`ArgsMaximum`, `ArgsLength`, maximum
/// count, offset 0, actual count) and the blocks.
#[must_use]
pub fn request_body(order: Order, args_max: u32, blocks: &[u8]) -> Vec<u8> {
    let len = u32::try_from(blocks.len()).unwrap_or(u32::MAX);
    let mut w = Writer::with_capacity(20 + blocks.len());
    w.u32_in(order, args_max).u32_in(order, len);
    w.u32_in(order, args_max).u32_in(order, 0).u32_in(order, len);
    w.bytes(blocks);
    w.into_inner()
}

/// Reads a request's body: `ArgsMaximum` and the blocks.
///
/// # Errors
/// Truncated, or an NDR header whose counts disagree (the checks of
/// p-net's `pf_get_ndr_data_req`).
pub fn parse_request_body(order: Order, body: &[u8]) -> Result<(u32, &[u8]), DecodeError> {
    let mut r = Reader::new(body);
    let args_max = r.u32_in(order)?;
    let args_len = r.u32_in(order)?;
    let max_count = r.u32_in(order)?;
    let offset = r.u32_in(order)?;
    let actual = r.u32_in(order)?;
    if args_max < args_len
        || args_max < max_count
        || max_count < args_len
        || args_len != actual
        || offset != 0
    {
        return Err(DecodeError::Invalid("NDR request header"));
    }
    let len = usize::try_from(args_len).map_err(|_| DecodeError::Invalid("NDR length"))?;
    Ok((args_max, r.bytes(len)?))
}

/// A response's body: the status, then the NDR counts and the blocks.
#[must_use]
pub fn response_body(order: Order, status: Status, max_count: u32, blocks: &[u8]) -> Vec<u8> {
    let len = u32::try_from(blocks.len()).unwrap_or(u32::MAX);
    let mut w = Writer::with_capacity(20 + blocks.len());
    w.u32_in(order, status.to_u32()).u32_in(order, len);
    w.u32_in(order, max_count).u32_in(order, 0).u32_in(order, len);
    w.bytes(blocks);
    w.into_inner()
}

/// Reads a response's body: its status and the blocks.
///
/// # Errors
/// Truncated, or counts that disagree with what follows.
pub fn parse_response_body(order: Order, body: &[u8]) -> Result<(Status, &[u8]), DecodeError> {
    let mut r = Reader::new(body);
    let status = Status::from_u32(r.u32_in(order)?);
    let args_len = r.u32_in(order)?;
    r.u32_in(order)?;
    r.u32_in(order)?;
    let actual = r.u32_in(order)?;
    if args_len != actual {
        return Err(DecodeError::Invalid("NDR response header"));
    }
    let len = usize::try_from(args_len).map_err(|_| DecodeError::Invalid("NDR length"))?;
    Ok((status, r.bytes(len)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(order: Order) -> Header {
        Header {
            packet_type: PT_REQUEST,
            flags: FLAG_IDEMPOTENT,
            order,
            object: object_uuid(1, 0x0A01, 0x0145),
            interface: IF_DEVICE,
            activity: Uuid::new(0x0102_0304, 0x0506, 0x0708, [9, 10, 11, 12, 13, 14, 15, 16]),
            server_boot: 0,
            sequence: 3,
            opnum: OP_CONNECT,
            fragment: 0,
        }
    }

    #[test]
    fn the_header_is_eighty_bytes_in_either_order() {
        for order in [Order::Big, Order::Little] {
            let h = header(order);
            let bytes = h.encode(&[0xAA; 5]).unwrap();
            assert_eq!(bytes.len(), HEADER_LEN + 5);
            let (back, body) = Header::decode(&bytes).unwrap();
            assert_eq!(back, h);
            assert_eq!(body, [0xAA; 5]);
        }
    }

    #[test]
    fn a_little_endian_header_swaps_the_uuid_fields_and_integers() {
        let bytes = header(Order::Little).encode(&[]).unwrap();
        assert_eq!(&bytes[..8], &[4, 0, 0x20, 0, 0x10, 0, 0, 0]);
        // Object UUID dea00000-6c97-11d1-8271-0001 0a01 0145.
        assert_eq!(
            &bytes[8..24],
            &[
                0x00, 0x00, 0xA0, 0xDE, 0x97, 0x6C, 0xD1, 0x11, 0x82, 0x71, 0x00, 0x01, 0x0A, 0x01, 0x01,
                0x45
            ]
        );
        // Interface version 1, sequence 3, opnum 0, hints, length 0.
        assert_eq!(&bytes[60..64], &[1, 0, 0, 0]);
        assert_eq!(&bytes[64..68], &[3, 0, 0, 0]);
        assert_eq!(&bytes[68..76], &[0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0]);
        let big = header(Order::Big).encode(&[]).unwrap();
        assert_eq!(&big[8..12], &[0xDE, 0xA0, 0x00, 0x00]);
        assert_eq!(big[4], 0);
    }

    #[test]
    fn uuids_print_in_the_usual_notation() {
        assert_eq!(IF_DEVICE.to_string(), "dea00001-6c97-11d1-8271-00a02442df7d");
        assert!(is_object_uuid(&object_uuid(1, 2, 3)));
        assert!(!is_object_uuid(&IF_DEVICE));
    }

    #[test]
    fn bodies_read_back_and_bad_counts_are_refused() {
        for order in [Order::Big, Order::Little] {
            let req = request_body(order, 1000, &[1, 2, 3]);
            assert_eq!(parse_request_body(order, &req).unwrap(), (1000, &[1u8, 2, 3][..]));
            let status = Status {
                code: 0xDB,
                decode: 0x81,
                code1: 0x01,
                code2: 0x02,
            };
            let res = response_body(order, status, 1000, &[4, 5]);
            assert_eq!(parse_response_body(order, &res).unwrap(), (status, &[4u8, 5][..]));
        }
        let mut bad = request_body(Order::Big, 1000, &[1, 2, 3]);
        bad[19] = 2;
        assert_eq!(
            parse_request_body(Order::Big, &bad),
            Err(DecodeError::Invalid("NDR request header"))
        );
        // The status travels as an integer: little-endian puts code2 first.
        let res = response_body(Order::Little, Status::from_u32(0xDB81_0102), 0, &[]);
        assert_eq!(&res[..4], &[0x02, 0x01, 0x81, 0xDB]);
    }

    #[test]
    fn a_body_too_long_or_a_short_datagram_is_an_error() {
        assert!(header(Order::Big).encode(&[0; MAX_BODY + 1]).is_err());
        let bytes = header(Order::Big).encode(&[0; 4]).unwrap();
        assert!(matches!(
            Header::decode(&bytes[..82]),
            Err(DecodeError::Truncated { .. })
        ));
        let mut v5 = bytes;
        v5[0] = 5;
        assert!(Header::decode(&v5).is_err());
    }
}
