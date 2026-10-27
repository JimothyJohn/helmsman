//! Common Packet Format (CPF): an item count followed by type/length/data
//! items. Used inside `ListIdentity`, `SendRRData` and `SendUnitData`.
//!
//! Item type IDs from `OpENer` `cpf.h` (`CipItemId`); parsing mirrors
//! `CreateCommonPacketFormatStructure` in `cpf.c` but keeps every item as
//! raw bytes instead of assuming the address/data/sockaddr shape.

use crate::codec::{DecodeError, Reader, Writer};

/// CPF item type IDs.
pub mod item {
    pub const NULL_ADDRESS: u16 = 0x0000;
    pub const LIST_IDENTITY_RESPONSE: u16 = 0x000C;
    pub const CONNECTION_ADDRESS: u16 = 0x00A1;
    pub const CONNECTED_DATA: u16 = 0x00B1;
    pub const UNCONNECTED_DATA: u16 = 0x00B2;
    pub const LIST_SERVICES_RESPONSE: u16 = 0x0100;
    pub const SOCKADDR_O_TO_T: u16 = 0x8000;
    pub const SOCKADDR_T_TO_O: u16 = 0x8001;
    pub const SEQUENCED_ADDRESS: u16 = 0x8002;
}

/// One item: its type and its body, exactly as long as the item declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item<'a> {
    pub type_id: u16,
    pub data: &'a [u8],
}

/// Parse every item. Trailing bytes after the last declared item are an
/// error: a length that doesn't add up is how a malformed packet hides.
///
/// # Errors
/// `Truncated` if the count or an item's declared length overruns `payload`;
/// `Invalid` if bytes remain after the last item.
pub fn decode(payload: &[u8]) -> Result<Vec<Item<'_>>, DecodeError> {
    let mut r = Reader::new(payload);
    let count = r.u16_le()?;
    let mut items = Vec::with_capacity(usize::from(count.min(8)));
    for _ in 0..count {
        let type_id = r.u16_le()?;
        let len = usize::from(r.u16_le()?);
        let data = r.bytes(len)?;
        items.push(Item { type_id, data });
    }
    if !r.is_empty() {
        return Err(DecodeError::Invalid("bytes after last CPF item"));
    }
    Ok(items)
}

/// Encode items with a count prefix.
///
/// # Panics
/// If there are more than `u16::MAX` items or one item exceeds `u16::MAX` bytes.
pub fn encode(items: &[Item<'_>], w: &mut Writer) {
    w.u16_le(u16::try_from(items.len()).expect("CPF item count exceeds u16"));
    for it in items {
        w.u16_le(it.type_id)
            .u16_le(u16::try_from(it.data.len()).expect("CPF item exceeds u16"))
            .bytes(it.data);
    }
}

/// The first item with `type_id`, if any.
#[must_use]
pub fn find<'a>(items: &[Item<'a>], type_id: u16) -> Option<Item<'a>> {
    items.iter().copied().find(|i| i.type_id == type_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_two_items() {
        let items = [
            Item {
                type_id: item::NULL_ADDRESS,
                data: &[],
            },
            Item {
                type_id: item::UNCONNECTED_DATA,
                data: b"\x0e\x02\x20\x01\x24\x01",
            },
        ];
        let mut w = Writer::default();
        encode(&items, &mut w);
        let bytes = w.into_inner();
        assert_eq!(&bytes[..2], &[2, 0]);
        assert_eq!(decode(&bytes).unwrap(), items);
        assert_eq!(find(&items, item::UNCONNECTED_DATA).unwrap().data.len(), 6);
        assert!(find(&items, item::CONNECTED_DATA).is_none());
    }

    #[test]
    fn trailing_bytes_rejected() {
        assert_eq!(
            decode(&[0, 0, 0xFF]),
            Err(DecodeError::Invalid("bytes after last CPF item"))
        );
    }

    #[test]
    fn declared_length_overrun_is_truncated() {
        assert!(matches!(
            decode(&[1, 0, 0xB2, 0, 10, 0, 1, 2]),
            Err(DecodeError::Truncated { .. })
        ));
    }
}
