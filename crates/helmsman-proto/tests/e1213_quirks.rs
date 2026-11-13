//! Replies captured from the Moxa ioLogik E1213 on the bench (2026-11-07,
//! `scratch/e1213-enip.pcap`, 192.168.3.60): what it really sends, held
//! as regression tests.
//!
//! A `Forward_Open` that carries a T->O socket-address item (the box's I/O
//! endpoint not on 2222) comes back with that item echoed after the
//! unconnected-data item, but the CPF item count says 2, not 3: 20 bytes
//! past the last item the count declares. The decoder refused the whole
//! reply ("bytes after last CPF item"), which hid what the E1213 was
//! saying: a refusal, general status 0x01, extended 0x0106 (ownership
//! conflict). A reply's trailing bytes are now accepted only when they are
//! whole socket-address items; anything else is still refused.

use helmsman_proto::encap::Frame;
use helmsman_proto::explicit::decode_send_rr_data_with_items;
use helmsman_proto::io::{extended, open_result};

/// The E1213's `SendRRData` reply to our `Forward_Open`, as captured.
const REPLY: &str = "6f0034000100000000000000020000000000000000000000000000000000020000000000b2001000d400010106010708ffff000000000000018010000002f89d000000000000000000000000";

fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn an_echoed_socket_address_item_past_the_count_is_read_and_the_refusal_shows() {
    let raw = bytes(REPLY);
    let (frame, after_frame) = Frame::decode(&raw).unwrap();
    assert_eq!(after_frame, b"", "one whole frame");
    let (reply, extra) = decode_send_rr_data_with_items(&frame).expect("the E1213's reply decodes");
    assert_eq!(extra.len(), 1, "the echoed item: {extra:?}");
    assert_eq!(extra[0].0, 0x8001, "T->O socket address");
    assert_eq!(extra[0].1.len(), 16);
    let refused = open_result(&reply)
        .unwrap()
        .expect_err("the E1213 refused the open");
    assert_eq!(refused.extended_code(), Some(extended::OWNERSHIP_CONFLICT));
    let text = refused.to_string();
    assert!(text.contains("ownership conflict"), "{text}");
}

#[test]
fn trailing_bytes_that_are_not_whole_socket_address_items_are_still_refused() {
    let raw = bytes(REPLY);
    // A data item instead of a socket address past the count.
    let mut not_sockaddr = raw.clone();
    let at = raw.len() - 20;
    not_sockaddr[at] = 0xb2;
    not_sockaddr[at + 1] = 0x00;
    // A socket-address item cut short, and one byte of garbage.
    let cut = &raw[..raw.len() - 1];
    let mut extra = raw.clone();
    extra.push(0x00);
    for (what, mut b) in [
        ("a data item", not_sockaddr),
        ("a cut item", cut.to_vec()),
        ("garbage after it", extra),
    ] {
        // The encapsulation length must match what follows the header.
        let len = u16::try_from(b.len() - 24).unwrap().to_le_bytes();
        b[2] = len[0];
        b[3] = len[1];
        let (frame, _) = Frame::decode(&b).unwrap();
        assert!(
            decode_send_rr_data_with_items(&frame).is_err(),
            "{what} must not be read as a reply"
        );
    }
}
