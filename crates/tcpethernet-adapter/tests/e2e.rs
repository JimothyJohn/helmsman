//! Adapter and originator from this repo talking over real loopback
//! sockets: discovery, explicit reads, Class 1 data both ways, timeouts,
//! and garbage on every port.

use std::io::Write;
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream, UdpSocket};
use std::thread;
use std::time::{Duration, Instant};

use proptest::prelude::*;
use tcpethernet::cip::class;
use tcpethernet::discover::sweep;
use tcpethernet::scanner::{IoConfig, IoConnection};
use tcpethernet::session::Session;
use tcpethernet_adapter::device::lock_ref;
use tcpethernet_adapter::{Adapter, Device, DeviceConfig, ServerConfig};

fn start(peer_io_port: u16) -> Adapter {
    let mut cfg = DeviceConfig::opener_sample();
    cfg.initial_watchdog = Duration::from_millis(500);
    Adapter::start(
        Device::new(cfg),
        &ServerConfig {
            bind: Ipv4Addr::LOCALHOST,
            encap_port: 0,
            io_port: 0,
            peer_io_port,
            ..ServerConfig::default()
        },
    )
    .unwrap()
}

fn wait_until(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if f() {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    false
}

#[test]
fn discovery_and_identity_over_real_sockets() {
    let a = start(1);
    let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let r = sweep(&sock, a.encap_addr(), Duration::from_millis(500)).unwrap();
    assert_eq!(r.found.len(), 1, "{r:?}");
    assert_eq!(r.found[0].identity.product_name, b"OpENer PC");
    assert_eq!(r.found[0].identity.socket_address.port, a.encap_addr().port());

    let mut s = Session::connect(a.encap_addr(), Duration::from_secs(2)).unwrap();
    assert_eq!(
        s.get_attribute_single(class::IDENTITY, 1, 1).unwrap(),
        1u16.to_le_bytes()
    );
    assert_eq!(a.sessions(), 1);
    s.close();
    assert!(wait_until(Duration::from_secs(2), || a.sessions() == 0));
}

#[test]
fn class1_both_directions_then_timeout() {
    let scanner_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let a = start(scanner_sock.local_addr().unwrap().port());
    let dev = a.device();
    let mut s = Session::connect(a.encap_addr(), Duration::from_secs(2)).unwrap();
    let cfg = IoConfig::exclusive_owner(151, 150, 100, 32, 32, Duration::from_millis(10));
    let conn = IoConnection::open(&mut s, &scanner_sock, a.io_addr(), cfg).unwrap();

    // O->T: scanner output lands in assembly 150.
    conn.set_output(&[0x5A; 32]).unwrap();
    assert!(
        wait_until(Duration::from_secs(2), || lock_ref(&dev).assembly(150)
            == Some(&[0x5A; 32][..])),
        "assembly 150 never got the scanner's output"
    );
    assert!(wait_until(Duration::from_secs(2), || lock_ref(&dev).scanner_running()));

    // T->O: the application writes assembly 100, the scanner sees it.
    lock_ref(&dev).write_assembly(100, &[0xA5; 32]).unwrap();
    let got = conn
        .wait_for_input_eq(&[0xA5; 32], Duration::from_secs(2))
        .expect("no T->O");
    assert_eq!(got.data, vec![0xA5; 32]);
    assert_eq!(conn.packets_ignored(), 0);

    // Stop producing without a Forward_Close: the adapter must time it out
    // (10 ms x 2^3 = 80 ms) and drop ownership.
    drop(conn);
    assert!(
        wait_until(Duration::from_secs(2), || lock_ref(&dev).connections().is_empty()),
        "connection never timed out"
    );
    assert!(!lock_ref(&dev).scanner_running());
    assert_eq!(lock_ref(&dev).stats.timeouts, 1);

    // And it can be opened again.
    let cfg = IoConfig::exclusive_owner(151, 150, 100, 32, 32, Duration::from_millis(10));
    let conn = IoConnection::open(&mut s, &scanner_sock, a.io_addr(), cfg).unwrap();
    conn.close(&mut s).unwrap();
    assert_eq!(lock_ref(&dev).connections().len(), 0);
    s.close();
}

#[test]
fn garbage_on_every_port_does_not_stop_service() {
    let a = start(1);
    // TCP: a header claiming 65535 bytes then a hang-up; a bad options field; random junk.
    for junk in [
        vec![
            0x6F, 0x00, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
        vec![
            0x65, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0,
        ],
        (0..200u8).collect(),
    ] {
        let mut t = TcpStream::connect(a.encap_addr()).unwrap();
        let _ = t.write_all(&junk);
    }
    let u = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    for junk in [vec![], vec![0x63], (0..255u8).collect::<Vec<_>>()] {
        let _ = u.send_to(&junk, a.encap_addr());
        let _ = u.send_to(&junk, a.io_addr());
    }
    // Still serves a well-formed session.
    let mut s = Session::connect(a.encap_addr(), Duration::from_secs(2)).unwrap();
    assert_eq!(
        s.get_attribute_single(class::IDENTITY, 1, 3).unwrap(),
        65001u16.to_le_bytes()
    );
    s.close();
}

#[test]
fn unregistered_session_and_double_register() {
    use std::io::Read;
    use tcpethernet::encap::{Frame, Status};
    use tcpethernet::explicit;
    let a = start(1);
    let mut t = TcpStream::connect(a.encap_addr()).unwrap();
    t.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let req = tcpethernet::cip::Request::get_attribute_single(1, 1, 1);
    t.write_all(&explicit::send_rr_data(0xDEAD, [1; 8], 0, &req))
        .unwrap();
    let mut buf = [0u8; 64];
    let n = t.read(&mut buf).unwrap();
    let (f, _) = Frame::decode(&buf[..n]).unwrap();
    assert_eq!(f.header.status, Status::InvalidSessionHandle as u32);
    assert_eq!(f.header.sender_context, [1; 8]);

    t.write_all(&explicit::register_session([2; 8])).unwrap();
    let n = t.read(&mut buf).unwrap();
    let (f, _) = Frame::decode(&buf[..n]).unwrap();
    assert_eq!(f.header.status, 0);
    t.write_all(&explicit::register_session([3; 8])).unwrap();
    let n = t.read(&mut buf).unwrap();
    let (f, _) = Frame::decode(&buf[..n]).unwrap();
    assert_eq!(
        f.header.status,
        Status::InvalidCommand as u32,
        "second register on one socket"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    /// Arbitrary bytes as an explicit request never panic the device and
    /// always get a reply with the reply bit set.
    #[test]
    fn any_request_bytes_get_a_reply(bytes in proptest::collection::vec(any::<u8>(), 0..80)) {
        let mut d = Device::new(DeviceConfig::opener_sample());
        let r = tcpethernet_adapter::device::handle_bytes(&mut d, &bytes, Ipv4Addr::LOCALHOST, Instant::now());
        prop_assert!(r.service & 0x80 != 0);
    }

    #[test]
    fn any_io_datagram_is_dropped_or_accepted_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..120)) {
        let mut d = Device::new(DeviceConfig::opener_sample());
        let _ = d.on_io_packet(&bytes, Ipv4Addr::LOCALHOST, Instant::now());
    }
}

#[test]
fn adapter_binds_its_documented_defaults() {
    // Not binding them here (they may be taken); just pin the contract.
    let d = ServerConfig::default();
    assert_eq!((d.encap_port, d.io_port, d.peer_io_port), (44818, 2222, 2222));
    let _ = SocketAddrV4::new(Ipv4Addr::LOCALHOST, d.encap_port);
}
