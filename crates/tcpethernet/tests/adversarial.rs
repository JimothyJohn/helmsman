//! Property tests: no input, however shaped, makes a decoder panic, and every
//! encoder output decodes back to what was encoded.

use proptest::prelude::*;
use std::net::Ipv4Addr;
use tcpethernet::codec::{Reader, Writer};
use tcpethernet::discover::classify;
use tcpethernet::encap::{Command, Frame, Header};
use tcpethernet::identity::{IdentityItem, SocketAddress, decode_list_identity, encode_list_identity};

fn identity_strategy() -> impl Strategy<Value = IdentityItem> {
    let net = (any::<u16>(), any::<u16>(), any::<u16>(), any::<u32>());
    let ident = (
        any::<u16>(),
        any::<u16>(),
        any::<u16>(),
        any::<u8>(),
        any::<u8>(),
        any::<u16>(),
        any::<u32>(),
    );
    let tail = (proptest::collection::vec(any::<u8>(), 0..=255), any::<u8>());
    (net, ident, tail).prop_map(
        |(
            (protocol_version, family, port, addr),
            (vendor_id, device_type, product_code, revision_major, revision_minor, status, serial_number),
            (product_name, state),
        )| IdentityItem {
            protocol_version,
            socket_address: SocketAddress {
                family,
                port,
                addr: Ipv4Addr::from(addr),
            },
            vendor_id,
            device_type,
            product_code,
            revision_major,
            revision_minor,
            status,
            serial_number,
            product_name,
            state,
        },
    )
}

proptest! {
    #[test]
    fn header_decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
        let _ = Header::decode(&mut Reader::new(&bytes));
    }

    #[test]
    fn frame_decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..200)) {
        let _ = Frame::decode(&bytes);
    }

    #[test]
    fn list_identity_decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
        let _ = decode_list_identity(&bytes);
    }

    #[test]
    fn classify_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..400), ctx in any::<[u8; 8]>()) {
        let _ = classify(&bytes, ctx);
    }

    #[test]
    fn every_prefix_of_a_valid_reply_is_truncated_not_panic(items in proptest::collection::vec(identity_strategy(), 0..4)) {
        let payload = encode_list_identity(&items);
        let msg = Frame::encode(Command::ListIdentity, 0, [7; 8], &payload);
        for cut in 0..msg.len() {
            let r = Frame::decode(&msg[..cut]);
            prop_assert!(matches!(r, Err(tcpethernet::DecodeError::Truncated { .. })), "cut={cut} gave {r:?}");
        }
        let (frame, rest) = Frame::decode(&msg).unwrap();
        prop_assert!(rest.is_empty());
        prop_assert_eq!(decode_list_identity(frame.payload).unwrap(), items);
    }

    #[test]
    fn header_round_trip(command in any::<u16>(), length in any::<u16>(), session in any::<u32>(), status in any::<u32>(), ctx in any::<[u8; 8]>()) {
        let h = Header { command, length, session_handle: session, status, sender_context: ctx, options: 0 };
        let back = Header::decode(&mut Reader::new(&h.encode())).unwrap();
        prop_assert_eq!(back, h);
    }

    #[test]
    fn identity_round_trip(item in identity_strategy()) {
        let mut w = Writer::default();
        item.encode(&mut w);
        let bytes = w.into_inner();
        let back = IdentityItem::decode(&mut Reader::new(&bytes)).unwrap();
        prop_assert_eq!(back, item);
    }

    #[test]
    fn classify_rejects_foreign_context(item in identity_strategy(), ours in any::<[u8; 8]>(), theirs in any::<[u8; 8]>()) {
        prop_assume!(ours != theirs);
        let msg = Frame::encode(Command::ListIdentity, 0, theirs, &encode_list_identity(&[item]));
        prop_assert_eq!(classify(&msg, ours), Err(tcpethernet::discover::Rejected::NotOurs));
    }
}
