//! `ListIdentity` response: Common Packet Format (CPF) items with the
//! Identity-object snapshot every device must answer with.
//!
//! Item IDs from `OpENer` `cpf.h` (`CipItemId`). The 0x000C item body follows
//! `OpENer` `encap.c` `EncodeListIdentityCipIdentityItem`: protocol version u16,
//! socket address (`sin_family` u16, `sin_port` u16, `sin_addr` u32, all
//! big-endian, then 8 zero bytes; `EncapsulateIpAddress` in `endianconv.c`),
//! vendor id u16, device type u16, product code u16, revision major u8 and
//! minor u8, status u16, serial u32, product name as `SHORT_STRING` (u8 length
//! then bytes), state u8. The Identity attribute set matches the mandatory
//! list in Rockwell ENET-WP001 §IV.B and PUB00213R0 p.18 ("Select a device
//! identity").

use std::net::Ipv4Addr;

use crate::codec::{DecodeError, Reader, Writer};

pub use crate::cpf::item;

/// BSD `sockaddr_in` as carried on the wire: 16 bytes, network byte order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocketAddress {
    /// `AF_INET`, which is 2 on every platform that matters.
    pub family: u16,
    pub port: u16,
    pub addr: Ipv4Addr,
}

impl SocketAddress {
    pub const LEN: usize = 16;
    pub const AF_INET: u16 = 2;

    /// # Errors
    /// `Truncated` if fewer than 16 bytes remain.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let family = r.u16_be()?;
        let port = r.u16_be()?;
        let addr = Ipv4Addr::from(r.u32_be()?);
        let _zero = r.bytes(8)?;
        Ok(Self { family, port, addr })
    }

    pub fn encode(&self, w: &mut Writer) {
        w.u16_be(self.family)
            .u16_be(self.port)
            .u32_be(self.addr.into())
            .bytes(&[0u8; 8]);
    }
}

/// Identity object attributes 1–8 as reported in a `ListIdentity` reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityItem {
    pub protocol_version: u16,
    pub socket_address: SocketAddress,
    pub vendor_id: u16,
    pub device_type: u16,
    pub product_code: u16,
    pub revision_major: u8,
    pub revision_minor: u8,
    pub status: u16,
    pub serial_number: u32,
    /// Raw `SHORT_STRING` bytes. Not guaranteed to be UTF-8.
    pub product_name: Vec<u8>,
    pub state: u8,
}

impl IdentityItem {
    /// Decode the body of a 0x000C item. `r` must be bounded to the item's
    /// declared length (see [`Reader::sub`]) so a lying length field cannot
    /// pull bytes from the next item.
    ///
    /// # Errors
    /// `Truncated` on any short field, including a product-name length that
    /// exceeds the item.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let protocol_version = r.u16_le()?;
        let socket_address = SocketAddress::decode(r)?;
        let vendor_id = r.u16_le()?;
        let device_type = r.u16_le()?;
        let product_code = r.u16_le()?;
        let revision_major = r.u8()?;
        let revision_minor = r.u8()?;
        let status = r.u16_le()?;
        let serial_number = r.u32_le()?;
        let name_len = usize::from(r.u8()?);
        let product_name = r.bytes(name_len)?.to_vec();
        let state = r.u8()?;
        Ok(Self {
            protocol_version,
            socket_address,
            vendor_id,
            device_type,
            product_code,
            revision_major,
            revision_minor,
            status,
            serial_number,
            product_name,
            state,
        })
    }

    /// Encode the item body (no type/length prefix).
    ///
    /// # Panics
    /// If `product_name` is longer than 255 bytes, which `SHORT_STRING` cannot carry.
    pub fn encode(&self, w: &mut Writer) {
        let name_len = u8::try_from(self.product_name.len()).expect("product name exceeds SHORT_STRING");
        w.u16_le(self.protocol_version);
        self.socket_address.encode(w);
        w.u16_le(self.vendor_id)
            .u16_le(self.device_type)
            .u16_le(self.product_code)
            .u8(self.revision_major)
            .u8(self.revision_minor)
            .u16_le(self.status)
            .u32_le(self.serial_number)
            .u8(name_len)
            .bytes(&self.product_name)
            .u8(self.state);
    }

    /// Product name as text, lossily.
    #[must_use]
    pub fn product_name_lossy(&self) -> String {
        String::from_utf8_lossy(&self.product_name).into_owned()
    }
}

/// Parse the payload of a `ListIdentity` reply: item count, then items.
/// Items other than 0x000C are skipped. Returns every identity item found;
/// a device answers with exactly one, but a tolerant parser costs nothing.
///
/// # Errors
/// `Truncated` if the count or any item's declared length overruns the
/// payload; `Invalid` if bytes trail the last item.
pub fn decode_list_identity(payload: &[u8]) -> Result<Vec<IdentityItem>, DecodeError> {
    let mut out = Vec::new();
    for it in crate::cpf::decode(payload)? {
        if it.type_id == item::LIST_IDENTITY_RESPONSE {
            out.push(IdentityItem::decode(&mut Reader::new(it.data))?);
        }
    }
    Ok(out)
}

/// Encode a `ListIdentity` reply payload carrying one identity item. Used by
/// the test adapter and by anything that needs to answer discovery.
#[must_use]
pub fn encode_list_identity(items: &[IdentityItem]) -> Vec<u8> {
    let mut w = Writer::default();
    w.u16_le(u16::try_from(items.len()).expect("item count exceeds u16"));
    for it in items {
        let mut body = Writer::default();
        it.encode(&mut body);
        let body = body.into_inner();
        w.u16_le(item::LIST_IDENTITY_RESPONSE)
            .u16_le(u16::try_from(body.len()).expect("item exceeds u16"))
            .bytes(&body);
    }
    w.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> IdentityItem {
        IdentityItem {
            protocol_version: 1,
            socket_address: SocketAddress {
                family: SocketAddress::AF_INET,
                port: crate::encap::PORT,
                addr: Ipv4Addr::new(192, 168, 1, 10),
            },
            vendor_id: 1,
            device_type: 0x0C,
            product_code: 65001,
            revision_major: 1,
            revision_minor: 3,
            status: 0x0030,
            serial_number: 0x1234_5678,
            product_name: b"OpENer PC".to_vec(),
            state: 3,
        }
    }

    #[test]
    fn socket_address_is_big_endian() {
        let mut w = Writer::default();
        sample().socket_address.encode(&mut w);
        let b = w.into_inner();
        assert_eq!(&b[..8], &[0, 2, 0xAF, 0x12, 192, 168, 1, 10]);
        assert_eq!(&b[8..], &[0u8; 8]);
    }

    #[test]
    fn round_trip_one_item() {
        let payload = encode_list_identity(&[sample()]);
        assert_eq!(&payload[..2], &[1, 0]);
        assert_eq!(&payload[2..4], &[0x0C, 0]);
        let items = decode_list_identity(&payload).unwrap();
        assert_eq!(items, vec![sample()]);
    }

    #[test]
    fn unknown_items_are_skipped() {
        let mut w = Writer::default();
        w.u16_le(2)
            .u16_le(item::LIST_SERVICES_RESPONSE)
            .u16_le(3)
            .bytes(b"xyz");
        let mut body = Writer::default();
        sample().encode(&mut body);
        let body = body.into_inner();
        w.u16_le(item::LIST_IDENTITY_RESPONSE)
            .u16_le(u16::try_from(body.len()).unwrap())
            .bytes(&body);
        let items = decode_list_identity(&w.into_inner()).unwrap();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn item_length_cannot_borrow_from_next_item() {
        // Item 1 declares 4 bytes fewer than its body; the leftover 4 bytes
        // are a valid second item. Decoding item 1 must be Truncated, never
        // a read into item 2.
        let mut body = Writer::default();
        sample().encode(&mut body);
        let body = body.into_inner();
        let cut = body.len() - 4;
        let mut w = Writer::default();
        w.u16_le(2)
            .u16_le(item::LIST_IDENTITY_RESPONSE)
            .u16_le(u16::try_from(cut).unwrap())
            .bytes(&body[..cut])
            .u16_le(item::NULL_ADDRESS)
            .u16_le(0);
        assert!(matches!(
            decode_list_identity(&w.into_inner()),
            Err(DecodeError::Truncated { .. })
        ));
    }

    #[test]
    fn huge_item_count_with_no_data_is_truncated() {
        assert!(matches!(
            decode_list_identity(&[0xFF, 0xFF]),
            Err(DecodeError::Truncated { .. })
        ));
    }
}
