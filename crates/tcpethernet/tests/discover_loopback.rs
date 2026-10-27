//! End-to-end discovery against a real UDP socket on loopback that answers
//! the way an adapter does. No mocks: bytes go through the kernel.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::thread;
use std::time::Duration;

use tcpethernet::discover::{Rejected, sweep};
use tcpethernet::encap::{Command, Frame, HEADER_LEN};
use tcpethernet::identity::{IdentityItem, SocketAddress, encode_list_identity};

fn adapter_identity(port: u16) -> IdentityItem {
    IdentityItem {
        protocol_version: 1,
        socket_address: SocketAddress {
            family: SocketAddress::AF_INET,
            port,
            addr: Ipv4Addr::LOCALHOST,
        },
        vendor_id: 1,
        device_type: 0x0C,
        product_code: 65001,
        revision_major: 2,
        revision_minor: 3,
        status: 0x0030,
        serial_number: 0xCAFE_F00D,
        product_name: b"loopback adapter".to_vec(),
        state: 3,
    }
}

/// Bind a fake adapter on loopback. It answers the first `ListIdentity` with a
/// well-formed reply plus, if `also_garbage`, one datagram of junk, then exits.
fn spawn_adapter(also_garbage: bool) -> (SocketAddrV4, thread::JoinHandle<()>) {
    let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let addr = match sock.local_addr().unwrap() {
        std::net::SocketAddr::V4(a) => a,
        std::net::SocketAddr::V6(_) => unreachable!(),
    };
    sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let handle = thread::spawn(move || {
        let mut buf = [0u8; 1500];
        let (n, from) = sock.recv_from(&mut buf).expect("adapter never got a request");
        let (frame, _) = Frame::decode(&buf[..n]).expect("request did not decode");
        assert_eq!(frame.header.command, Command::ListIdentity as u16);
        assert_eq!(frame.header.length, 0);
        let identity = adapter_identity(addr.port());
        let reply = Frame::encode(
            Command::ListIdentity,
            0,
            frame.header.sender_context,
            &encode_list_identity(&[identity]),
        );
        if also_garbage {
            sock.send_to(&reply[..HEADER_LEN - 3], from).unwrap();
        }
        sock.send_to(&reply, from).unwrap();
    });
    (addr, handle)
}

#[test]
fn discovers_a_loopback_adapter() {
    let (adapter, handle) = spawn_adapter(false);
    let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let sweep_result = sweep(&sock, adapter, Duration::from_millis(500)).unwrap();
    handle.join().unwrap();
    assert_eq!(sweep_result.rejected, vec![], "{sweep_result:?}");
    assert_eq!(sweep_result.found.len(), 1, "{sweep_result:?}");
    let found = &sweep_result.found[0];
    assert_eq!(found.from, std::net::SocketAddr::V4(adapter));
    assert_eq!(found.identity, adapter_identity(adapter.port()));
    assert_eq!(found.identity.product_name_lossy(), "loopback adapter");
}

#[test]
fn garbage_datagram_is_reported_not_fatal() {
    let (adapter, handle) = spawn_adapter(true);
    let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let sweep_result = sweep(&sock, adapter, Duration::from_millis(500)).unwrap();
    handle.join().unwrap();
    assert_eq!(sweep_result.found.len(), 1, "{sweep_result:?}");
    assert_eq!(sweep_result.rejected.len(), 1, "{sweep_result:?}");
    assert!(
        matches!(sweep_result.rejected[0].1, Rejected::BadFrame(_)),
        "{sweep_result:?}"
    );
}

#[test]
fn silence_returns_empty_after_window() {
    // A port we bound and released: guaranteed nothing listening.
    let probe = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let dead = match probe.local_addr().unwrap() {
        std::net::SocketAddr::V4(a) => a,
        std::net::SocketAddr::V6(_) => unreachable!(),
    };
    drop(probe);
    let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let start = std::time::Instant::now();
    let sweep_result = sweep(&sock, dead, Duration::from_millis(200)).unwrap();
    assert!(start.elapsed() >= Duration::from_millis(200));
    assert_eq!(sweep_result, tcpethernet::discover::Sweep::default());
}
