//! The interface calls against the loopback interface, which every host
//! has: its address and mask, and a UDP socket tied to it.

#![cfg(unix)]

use std::net::UdpSocket;

#[cfg(any(target_os = "macos", target_os = "ios"))]
const LOOPBACK: &str = "lo0";
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
const LOOPBACK: &str = "lo";

#[test]
fn loopback_has_127_0_0_1_in_a_slash_8() {
    let (addr, mask) = helmsman_l2::ipv4_net(LOOPBACK)
        .unwrap()
        .expect("loopback has IPv4");
    assert_eq!(addr, [127, 0, 0, 1]);
    assert_eq!(mask, [255, 0, 0, 0]);
    assert_eq!(helmsman_l2::ipv4_net("no-such-if0").unwrap(), None);
}

#[test]
fn a_socket_tied_to_loopback_still_talks_on_loopback() {
    let a = UdpSocket::bind("127.0.0.1:0").unwrap();
    let b = UdpSocket::bind("127.0.0.1:0").unwrap();
    helmsman_l2::bind_to_interface(&a, LOOPBACK).unwrap();
    a.send_to(b"hi", b.local_addr().unwrap()).unwrap();
    let mut buf = [0u8; 8];
    let (n, from) = b.recv_from(&mut buf).unwrap();
    assert_eq!((&buf[..n], from), (&b"hi"[..], a.local_addr().unwrap()));
}

#[test]
fn an_unknown_interface_is_an_error() {
    let a = UdpSocket::bind("127.0.0.1:0").unwrap();
    assert!(helmsman_l2::bind_to_interface(&a, "no-such-if0").is_err());
    assert!(helmsman_l2::bind_to_interface(&a, "bad\0name").is_err());
}
