//! A box on one Class 1 I/O port: its adapter (owned by a scanner) and its
//! own originator connection to a device, both on the same UDP socket,
//! both flowing, neither disturbed by the other or by junk.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use helmsman_adapter::device::lock_ref;
use helmsman_adapter::{Adapter, Device, DeviceConfig, ServerConfig};
use helmsman_proto::endpoint::IoEndpoint;
use helmsman_proto::io::IoPacket;
use helmsman_proto::scanner::{IoConfig, IoConnection};
use helmsman_proto::session::Session;

const RPI: Duration = Duration::from_millis(10);

fn server_cfg() -> ServerConfig {
    ServerConfig {
        bind: Ipv4Addr::LOCALHOST,
        encap_port: 0,
        io_port: 0,
        peer_io_port: 0,
        ..ServerConfig::default()
    }
}

fn device_cfg() -> DeviceConfig {
    let mut cfg = DeviceConfig::opener_sample();
    cfg.initial_watchdog = Duration::from_millis(500);
    cfg
}

fn io_cfg() -> IoConfig {
    IoConfig::exclusive_owner(151, 150, 100, 32, 32, RPI)
}

fn wait(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(2));
    }
}

fn v4(a: SocketAddr) -> SocketAddrV4 {
    match a {
        SocketAddr::V4(a) => a,
        SocketAddr::V6(_) => panic!("IPv6"),
    }
}

/// The box (adapter + originator on one endpoint), a device it masters,
/// and a scanner that owns the box.
struct Plant {
    ep: IoEndpoint,
    the_box: Adapter,
    device: Adapter,
    scanner_sock: UdpSocket,
    scanner_session: Session,
    scanner: IoConnection,
    master_session: Session,
    master: IoConnection,
}

impl Plant {
    fn new() -> Self {
        let ep = IoEndpoint::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
        let the_box = Adapter::start_on(Device::new(device_cfg()), &server_cfg(), &ep).unwrap();
        assert_eq!(
            the_box.io_addr(),
            ep.local_addr(),
            "the adapter consumes on the shared endpoint"
        );
        let device = Adapter::start(Device::new(device_cfg()), &server_cfg()).unwrap();

        let scanner_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut scanner_session = Session::connect(the_box.encap_addr(), Duration::from_secs(2)).unwrap();
        let scanner =
            IoConnection::open(&mut scanner_session, &scanner_sock, the_box.io_addr(), io_cfg()).unwrap();

        let mut master_session = Session::connect(device.encap_addr(), Duration::from_secs(2)).unwrap();
        let master =
            IoConnection::open_on(&mut master_session, &ep, device.io_addr(), io_cfg(), &[0; 32]).unwrap();
        Self {
            ep,
            the_box,
            device,
            scanner_sock,
            scanner_session,
            scanner,
            master_session,
            master,
        }
    }

    /// Close the box's device connection (exclusive ownership goes), open
    /// a new one; returns the old one's T->O ID.
    fn reopen_master(&mut self) -> u32 {
        let new = |s: &mut Self| {
            IoConnection::open_on(
                &mut s.master_session,
                &s.ep,
                s.device.io_addr(),
                io_cfg(),
                &[0x22; 32],
            )
            .unwrap_or_else(|e| panic!("reopen: {e}"))
        };
        let old_id = self.master.reply().t_to_o_connection_id;
        // Close first: the device takes one exclusive owner at a time.
        self.master.closer().send(&mut self.master_session).unwrap();
        let fresh = new(self);
        drop(std::mem::replace(&mut self.master, fresh));
        old_id
    }

    /// Close the scanner's connection to the box, open a new one from a
    /// new socket; returns the old one's O->T ID.
    fn reopen_scanner(&mut self) -> u32 {
        let old_id = self.scanner.reply().o_to_t_connection_id;
        self.scanner.closer().send(&mut self.scanner_session).unwrap();
        let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let fresh =
            IoConnection::open(&mut self.scanner_session, &sock, self.the_box.io_addr(), io_cfg()).unwrap();
        drop(std::mem::replace(&mut self.scanner, fresh));
        self.scanner_sock = sock;
        old_id
    }
}

#[test]
fn adapter_and_originator_flow_on_one_port() {
    let p = Plant::new();
    // Scanner -> box adapter (O->T), box -> device (O->T).
    p.scanner.set_output(&[0x11; 32]).unwrap();
    p.master.set_output(&[0x22; 32]).unwrap();
    let box_dev = p.the_box.device();
    let dev = p.device.device();
    wait("the scanner's output in the box", || {
        lock_ref(&box_dev).assembly(150) == Some(&[0x11; 32][..])
    });
    wait("the box's output in the device", || {
        lock_ref(&dev).assembly(150) == Some(&[0x22; 32][..])
    });
    // Device -> box originator (T->O), box adapter -> scanner (T->O).
    lock_ref(&dev).write_assembly(100, &[0x33; 32]).unwrap();
    lock_ref(&box_dev).write_assembly(100, &[0x44; 32]).unwrap();
    assert!(
        p.master
            .wait_for_input_eq(&[0x33; 32], Duration::from_secs(3))
            .is_some_and(|i| i.data == [0x33; 32])
    );
    assert!(
        p.scanner
            .wait_for_input_eq(&[0x44; 32], Duration::from_secs(3))
            .is_some_and(|i| i.data == [0x44; 32])
    );

    // One port: the device sees the box's O->T come from the shared
    // endpoint, the box's adapter sees the scanner's from its socket.
    assert_eq!(p.device.o_to_t_sources(), vec![p.ep.local_addr()]);
    assert_eq!(
        p.the_box.o_to_t_sources(),
        vec![v4(p.scanner_sock.local_addr().unwrap())]
    );
    // Neither side ever saw the other's datagrams.
    assert_eq!(p.master.packets_ignored(), 0);
    assert_eq!(lock_ref(&box_dev).stats.io_dropped, 0);
    assert_eq!(p.ep.stats().wrong_peer, 0);
    // The IDs the box consumes are distinct and each routed to its owner.
    let adapter_id = lock_ref(&box_dev).connections()[0].o_to_t_id;
    let master_id = p.master.reply().t_to_o_connection_id;
    assert_ne!(adapter_id, master_id);
    let (a, m) = (
        p.ep.route_of(adapter_id).unwrap(),
        p.ep.route_of(master_id).unwrap(),
    );
    assert_ne!(a.owner, m.owner);
}

#[test]
fn foreign_and_unknown_ids_never_reach_the_other_owner() {
    let p = Plant::new();
    let box_dev = p.the_box.device();
    wait("both flowing", || {
        p.master.packets_received() > 3 && p.scanner.packets_received() > 3
    });
    let adapter_id = lock_ref(&box_dev).connections()[0].o_to_t_id;
    let master_id = p.master.reply().t_to_o_connection_id;
    let before = p.ep.stats();
    let dropped_before = lock_ref(&box_dev).stats.io_dropped;

    let junk = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let to = p.ep.local_addr();
    // Unknown IDs: dropped at the endpoint, counted, seen by nobody.
    for id in 0..100u32 {
        let pkt = IoPacket {
            connection_id: 0x7700_0000 + id,
            encapsulation_sequence: Some(1),
            body: &[0; 34],
        }
        .encode();
        junk.send_to(&pkt, to).unwrap();
    }
    // Malformed: never routed.
    for n in 0..50u8 {
        junk.send_to(&vec![n; usize::from(n)], to).unwrap();
    }
    wait("150 dropped at the endpoint", || {
        let s = p.ep.stats();
        s.unknown - before.unknown >= 100 && s.malformed - before.malformed >= 50
    });
    assert_eq!(p.master.packets_ignored(), 0);
    assert_eq!(lock_ref(&box_dev).stats.io_dropped, dropped_before);

    // A stale datagram with the adapter's ID reaches the adapter (which
    // drops it as stale) and never the originator.
    let stale = IoPacket {
        connection_id: adapter_id,
        encapsulation_sequence: Some(0),
        body: &[0; 38],
    }
    .encode();
    for _ in 0..20 {
        junk.send_to(&stale, to).unwrap();
    }
    wait("the adapter dropped 20 stale", || {
        lock_ref(&box_dev).stats.io_dropped >= dropped_before + 20
    });
    assert_eq!(p.master.packets_ignored(), 0);
    assert_eq!(
        p.ep.route_of(master_id).map(|r| r.peer),
        Some(Ipv4Addr::LOCALHOST)
    );
}

#[test]
fn churn_on_one_side_leaves_the_other_flowing() {
    let mut p = Plant::new();
    wait("both flowing", || {
        p.master.packets_received() > 3 && p.scanner.packets_received() > 3
    });
    // The scanner closes and reopens its connection to the box, again and
    // again, while the box's own connection to the device keeps going.
    let mut gone = Vec::new();
    for round in 0..8 {
        let before = p.master.packets_received();
        gone.push(p.reopen_scanner());
        wait(&format!("round {round}: scanner input"), || {
            p.scanner.packets_received() > 2
        });
        wait(&format!("round {round}: master kept flowing"), || {
            p.master.packets_received() > before + 2
        });
        assert!(p.master.is_alive());
    }
    // And the other way round: the box reopens its device connection while
    // the scanner stays connected to the box.
    for round in 0..8 {
        let before = p.scanner.packets_received();
        gone.push(p.reopen_master());
        wait(&format!("round {round}: master input"), || {
            p.master.packets_received() > 2
        });
        wait(&format!("round {round}: scanner kept flowing"), || {
            p.scanner.packets_received() > before + 2
        });
        assert!(p.scanner.is_alive());
    }
    // Every closed connection's route is gone; the two live ones stay.
    for id in gone {
        assert_eq!(
            p.ep.route_of(id),
            None,
            "route 0x{id:08X} outlived its connection"
        );
    }
    assert!(p.ep.route_of(p.scanner.reply().o_to_t_connection_id).is_some());
    assert!(p.ep.route_of(p.master.reply().t_to_o_connection_id).is_some());
    assert_eq!(lock_ref(&p.the_box.device()).connections().len(), 1);
}

#[test]
fn a_junk_flood_starves_neither_side() {
    let p = Plant::new();
    wait("both flowing", || {
        p.master.packets_received() > 3 && p.scanner.packets_received() > 3
    });
    let box_dev = p.the_box.device();
    let adapter_id = lock_ref(&box_dev).connections()[0].o_to_t_id;
    let stop = Arc::new(AtomicBool::new(false));
    let flood = {
        let (stop, to) = (Arc::clone(&stop), p.ep.local_addr());
        thread::spawn(move || {
            let s = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            let mut n: u32 = 0;
            while !stop.load(Ordering::Relaxed) {
                n = n.wrapping_add(1);
                let id = match n % 3 {
                    0 => adapter_id,
                    1 => 0x6600_0000 | n,
                    _ => n,
                };
                let pkt = IoPacket {
                    connection_id: id,
                    encapsulation_sequence: Some(0),
                    body: &[0xA5; 38],
                }
                .encode();
                let _ = s.send_to(&pkt, to);
                let _ = s.send_to(&pkt[..n as usize % pkt.len()], to);
                if n.is_multiple_of(256) {
                    thread::yield_now();
                }
            }
            n
        })
    };
    // During the flood both sides keep receiving fresh data.
    for _ in 0..3 {
        let (m, s) = (p.master.packets_received(), p.scanner.packets_received());
        wait("the master during the flood", || {
            p.master.packets_received() > m + 5
        });
        wait("the scanner during the flood", || {
            p.scanner.packets_received() > s + 5
        });
    }
    stop.store(true, Ordering::Relaxed);
    let sent = flood.join().unwrap();
    assert!(sent > 1000, "the flood sent {sent}");
    let st = p.ep.stats();
    assert!(st.unknown + st.malformed > 1000, "{st:?}");
    assert!(p.master.is_alive());
    assert!(p.scanner.is_alive());
    assert_eq!(p.master.packets_ignored(), 0);
}

/// Off 2222, the originator names its port in a T->O socket-address item,
/// and a target that would otherwise send to its default port sends there.
#[test]
fn a_moved_endpoint_names_its_port_in_the_forward_open() {
    let ep = IoEndpoint::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
    assert_ne!(ep.local_addr().port(), helmsman_proto::encap::IO_PORT);
    // The device's default T->O port is a socket nobody reads.
    let black_hole = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let device = Adapter::start(
        Device::new(device_cfg()),
        &ServerConfig {
            peer_io_port: black_hole.local_addr().unwrap().port(),
            ..server_cfg()
        },
    )
    .unwrap();
    lock_ref(&device.device())
        .write_assembly(100, &[0x5C; 32])
        .unwrap();
    let mut s = Session::connect(device.encap_addr(), Duration::from_secs(2)).unwrap();
    let conn = IoConnection::open_on(&mut s, &ep, device.io_addr(), io_cfg(), &[0; 32]).unwrap();
    assert!(
        conn.wait_for_input_eq(&[0x5C; 32], Duration::from_secs(3))
            .is_some_and(|i| i.data == [0x5C; 32]),
        "T->O never reached the endpoint's port"
    );
    black_hole.set_nonblocking(true).unwrap();
    let mut buf = [0u8; 128];
    assert!(
        black_hole.recv_from(&mut buf).is_err(),
        "T->O went to the default port"
    );
}
