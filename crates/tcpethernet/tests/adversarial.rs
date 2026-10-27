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
        prop_assert_eq!(rest, []);
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

mod explicit_props {
    use proptest::prelude::*;
    use tcpethernet::cip::{Path, Request, Response};
    use tcpethernet::codec::{Reader, Writer};
    use tcpethernet::cpf;

    proptest! {
        #[test]
        fn cpf_decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..300)) {
            let _ = cpf::decode(&bytes);
        }

        #[test]
        fn request_decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
            let _ = Request::decode(&bytes);
        }

        #[test]
        fn response_decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
            let _ = Response::decode(&bytes);
        }

        #[test]
        fn path_decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..32), words in any::<u8>()) {
            let _ = Path::decode(&mut Reader::new(&bytes), words);
        }

        #[test]
        fn path_round_trip(class in any::<u16>(), instance in any::<u16>(), attribute in proptest::option::of(any::<u16>())) {
            let p = Path { class, instance, attribute };
            let bytes = p.encode();
            prop_assert_eq!(bytes.len() % 2, 0);
            let back = Path::decode(&mut Reader::new(&bytes), p.size_words()).unwrap();
            prop_assert_eq!(back, p);
        }

        #[test]
        fn request_round_trip(service in 0u8..0x80, class in any::<u16>(), instance in any::<u16>(), data in proptest::collection::vec(any::<u8>(), 0..64)) {
            let req = Request::new(service, Path::new(class, instance)).with_data(data);
            let mut w = Writer::default();
            req.encode(&mut w);
            prop_assert_eq!(Request::decode(&w.into_inner()).unwrap(), req);
        }

        #[test]
        fn response_round_trip(service in 0x80u8..=0xFF, status in any::<u8>(), add in proptest::collection::vec(any::<u16>(), 0..4), data in proptest::collection::vec(any::<u8>(), 0..64)) {
            let resp = Response { service, status: tcpethernet::cip::GeneralStatus(status), additional_status: add, data };
            let mut w = Writer::default();
            resp.encode(&mut w);
            prop_assert_eq!(Response::decode(&w.into_inner()).unwrap(), resp);
        }
    }
}

mod io_props {
    use proptest::prelude::*;
    use tcpethernet::codec::Reader;
    use tcpethernet::io::{
        Class1Body, ConnectionPath, ForwardClose, ForwardCloseReply, ForwardOpen, ForwardOpenReply, IoPacket,
        NetworkParams, Transport,
    };

    proptest! {
        #[test]
        fn forward_open_decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..120)) {
            let _ = ForwardOpen::decode(&bytes);
        }

        #[test]
        fn forward_close_decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
            let _ = ForwardClose::decode(&bytes);
        }

        #[test]
        fn replies_decode_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
            let _ = ForwardOpenReply::decode(&bytes);
            let _ = ForwardCloseReply::decode(&bytes);
        }

        #[test]
        fn io_packet_decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..200), header in any::<bool>()) {
            if let Ok(p) = IoPacket::decode(&bytes) {
                let _ = Class1Body::decode(p.body, header);
            }
        }

        #[test]
        fn connection_path_decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..64), words in any::<u8>()) {
            let _ = ConnectionPath::decode(&mut Reader::new(&bytes), words);
        }

        #[test]
        fn network_params_round_trip(v in any::<u16>()) {
            // Bit 12 is reserved: dropped on decode, written as zero.
            if let Ok(p) = NetworkParams::decode(v) {
                prop_assert_eq!(p.encode(), v & !0x1000);
            }
        }

        #[test]
        fn transport_round_trip(v in any::<u8>()) {
            if let Ok(t) = Transport::decode(v) {
                prop_assert_eq!(t.encode(), v);
            }
        }

        #[test]
        fn io_packet_round_trip(id in any::<u32>(), seq in proptest::option::of(any::<u32>()), body in proptest::collection::vec(any::<u8>(), 0..64)) {
            let p = IoPacket { connection_id: id, encapsulation_sequence: seq, body: &body };
            let bytes = p.encode();
            prop_assert_eq!(IoPacket::decode(&bytes).unwrap(), p);
        }
    }
}
