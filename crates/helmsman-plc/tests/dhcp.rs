//! The box's DHCP server (`crate::dhcp`) on loopback: a device asking for
//! an address, another server answering the probe or turning up later,
//! the box's own replies heard back, and the lease file across a restart.
//!
//! Loopback has no broadcast, so the test hands the service its sockets
//! and the addresses a box would broadcast to: the "device" socket stands
//! where 255.255.255.255:68 is, and the "rival" socket where
//! 255.255.255.255:67 is (the probe goes there).

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use helmsman_dhcp::message::{BOOTREPLY, BOOTREQUEST, Kind, opt};
use helmsman_dhcp::{Config, Mac, Message};
use helmsman_plc::dhcp::{Dhcp, PROBE_MAC, Settings, Status, Wire};

const SERVER: Ipv4Addr = Ipv4Addr::new(192, 168, 3, 1);
const RIVAL: Ipv4Addr = Ipv4Addr::new(192, 168, 3, 254);
const PROBE: Duration = Duration::from_millis(300);

fn folder(what: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "helmsman-dhcp-{what}-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn udp() -> UdpSocket {
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    s.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
    s
}

fn mac(n: u8) -> Mac {
    Mac([0x00, 0x07, 0x46, 0x00, 0x00, n])
}

struct Bench {
    dhcp: Dhcp,
    /// Where requests go: the service's server socket.
    server: SocketAddr,
    /// Where another server's replies would arrive: the watch socket.
    watch: SocketAddr,
    device: UdpSocket,
    rival: UdpSocket,
    dir: PathBuf,
    lines: Arc<std::sync::Mutex<Vec<String>>>,
}

fn start(dir: PathBuf) -> Bench {
    try_start(dir).unwrap()
}

fn try_start(dir: PathBuf) -> std::io::Result<Bench> {
    let (server, watch, device, rival) = (udp(), udp(), udp(), udp());
    let (server_at, watch_at) = (server.local_addr()?, watch.local_addr()?);
    let wire = Wire {
        server,
        watch,
        probe_to: rival.local_addr()?,
        broadcast_to: device.local_addr()?,
        client_port: device.local_addr()?.port(),
    };
    let config = Config {
        server: SERVER,
        netmask: Ipv4Addr::new(255, 255, 255, 0),
        router: None,
        pool: (Ipv4Addr::new(192, 168, 3, 100), Ipv4Addr::new(192, 168, 3, 199)),
        lease_s: 3600,
        reserved: BTreeMap::new(),
        bootp: true,
    };
    let lines = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = {
        let lines = Arc::clone(&lines);
        Arc::new(move |l: &str| lines.lock().unwrap().push(l.to_string()))
    };
    let settings = Settings {
        interface: "lo-test".into(),
        config,
        dir: dir.clone(),
    };
    let dhcp = Dhcp::start(settings, wire, PROBE, log)?;
    Ok(Bench {
        dhcp,
        server: server_at,
        watch: watch_at,
        device,
        rival,
        dir,
        lines,
    })
}

impl Bench {
    fn wait_for(&self, want: impl Fn(&Status) -> bool) -> Status {
        let t0 = Instant::now();
        loop {
            let s = self.dhcp.handle().status();
            if want(&s) || t0.elapsed() > Duration::from_secs(3) {
                return s;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn serving(&self) {
        assert_eq!(self.wait_for(|s| *s == Status::Serving), Status::Serving);
    }

    /// Send `m` from the device, and the reply it gets, if any.
    fn ask(&self, m: &Message) -> Option<Message> {
        self.device.send_to(&m.encode(), self.server).unwrap();
        let mut buf = [0u8; 1500];
        let (n, _) = self.device.recv_from(&mut buf).ok()?;
        Some(Message::parse(&buf[..n]).unwrap())
    }

    /// DISCOVER then REQUEST: the address `mac` ends up with.
    fn lease(&self, mac: Mac) -> Ipv4Addr {
        let offer = self.ask(&request(mac, Kind::Discover)).expect("an offer");
        assert_eq!(offer.kind(), Some(Kind::Offer));
        let mut r = request(mac, Kind::Request);
        r.set_option(opt::SERVER_ID, &SERVER.octets());
        r.set_option(opt::REQUESTED_IP, &offer.yiaddr.octets());
        let ack = self.ask(&r).expect("an answer");
        assert_eq!(ack.kind(), Some(Kind::Ack));
        ack.yiaddr
    }

    /// Another server's reply, sent to where the box listens for them.
    fn rival_offers(&self, server_id: Ipv4Addr) {
        let mut m = Message::empty(BOOTREPLY);
        m.yiaddr = Ipv4Addr::new(192, 168, 3, 50);
        m.set_option(opt::MESSAGE_TYPE, &[Kind::Offer.code()]);
        m.set_option(opt::SERVER_ID, &server_id.octets());
        self.rival.send_to(&m.encode(), self.watch).unwrap();
    }
}

fn request(mac: Mac, kind: Kind) -> Message {
    let mut m = Message::empty(BOOTREQUEST);
    m.xid = 0x5eed_0000 | u32::from(mac.0[5]);
    m.chaddr[..6].copy_from_slice(&mac.0);
    m.set_option(opt::MESSAGE_TYPE, &[kind.code()]);
    m
}

#[test]
fn nobody_else_answers_the_probe_so_the_box_serves_and_keeps_the_lease() {
    let b = start(folder("serve"));
    // The probe is a DISCOVER from the box's made-up address.
    let mut buf = [0u8; 1500];
    let (n, _) = b.rival.recv_from(&mut buf).expect("the probe");
    let probe = Message::parse(&buf[..n]).unwrap();
    assert_eq!(
        (probe.kind(), probe.mac()),
        (Some(Kind::Discover), Some(PROBE_MAC))
    );
    // Nothing is answered while it listens.
    assert_eq!(b.dhcp.handle().status(), Status::Probing);
    b.serving();
    assert_eq!(b.lease(mac(1)), Ipv4Addr::new(192, 168, 3, 100));
    let text = std::fs::read_to_string(b.dir.join("leases")).unwrap();
    assert!(text.contains("00-07-46-00-00-01 192.168.3.100 bound "), "{text}");
    let json = b.dhcp.handle().json();
    assert_eq!(json["status"], "serving");
    assert_eq!(json["leases"][0]["mac"], "00-07-46-00-00-01");
    assert_eq!(json["leases"][0]["state"], "bound");
    assert!(
        b.lines
            .lock()
            .unwrap()
            .iter()
            .any(|l| l == "dhcp: 00-07-46-00-00-01 has 192.168.3.100")
    );
}

#[test]
fn another_server_answering_the_probe_keeps_the_box_from_serving() {
    let b = start(folder("probe-refused"));
    let mut buf = [0u8; 1500];
    b.rival.recv_from(&mut buf).expect("the probe");
    b.rival_offers(RIVAL);
    let s = b.wait_for(|s| matches!(s, Status::Refused { .. }));
    assert_eq!(
        s,
        Status::Refused {
            server: RIVAL,
            offered: Ipv4Addr::new(192, 168, 3, 50)
        }
    );
    std::thread::sleep(PROBE);
    assert_eq!(
        b.ask(&request(mac(1), Kind::Discover)),
        None,
        "a refused box answers nobody"
    );
    assert_eq!(b.dhcp.handle().json()["refused_by"]["server"], "192.168.3.254");
    assert!(
        b.lines
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("another DHCP server answers") && l.contains("before this box served"))
    );
}

#[test]
fn another_server_turning_up_later_stops_the_box() {
    let b = start(folder("later"));
    b.serving();
    b.lease(mac(1));
    b.rival_offers(RIVAL);
    assert!(matches!(
        b.wait_for(|s| matches!(s, Status::Refused { .. })),
        Status::Refused { .. }
    ));
    assert_eq!(b.ask(&request(mac(2), Kind::Discover)), None);
}

#[test]
fn the_boxs_own_replies_heard_back_do_not_stop_it() {
    let b = start(folder("own"));
    b.serving();
    b.rival_offers(SERVER);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(b.dhcp.handle().status(), Status::Serving);
    assert!(b.ask(&request(mac(1), Kind::Discover)).is_some());
}

#[test]
fn a_restart_reads_the_lease_file_so_devices_keep_their_addresses() {
    let dir = folder("restart");
    {
        let b = start(dir.clone());
        b.serving();
        assert_eq!(b.lease(mac(1)), Ipv4Addr::new(192, 168, 3, 100));
    }
    let b = start(dir);
    assert_eq!(b.dhcp.handle().bindings().len(), 1, "restored before serving");
    b.serving();
    // Another device first: it does not get mac(1)'s address.
    assert_eq!(b.lease(mac(2)), Ipv4Addr::new(192, 168, 3, 101));
    assert_eq!(b.lease(mac(1)), Ipv4Addr::new(192, 168, 3, 100));
}

#[test]
fn a_lease_file_that_does_not_read_stops_the_start() {
    let dir = folder("corrupt");
    std::fs::write(dir.join("leases"), "# helmsman-dhcp leases 1\nnot a lease\n").unwrap();
    let err = try_start(dir).err().expect("refused");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("line 2"), "{err}");
}

#[test]
fn hostile_datagrams_are_dropped_and_the_box_keeps_serving() {
    let b = start(folder("hostile"));
    b.serving();
    for junk in [vec![], vec![0u8; 10], vec![0xffu8; 400], vec![1u8; 2000]] {
        b.device.send_to(&junk, b.server).unwrap();
    }
    // The probe's own address is never served.
    assert_eq!(b.ask(&request(PROBE_MAC, Kind::Discover)), None);
    assert_eq!(b.lease(mac(1)), Ipv4Addr::new(192, 168, 3, 100));
}

#[test]
fn pool_flags_parse_and_the_default_pool_is_only_for_a_slash_24() {
    use helmsman_plc::dhcp::{default_pool, parse_pool};
    assert_eq!(
        parse_pool("192.168.3.100-192.168.3.150"),
        Ok((Ipv4Addr::new(192, 168, 3, 100), Ipv4Addr::new(192, 168, 3, 150)))
    );
    for bad in [
        "192.168.3.100",
        "192.168.3.100-",
        "a-b",
        "192.168.3.100-192.168.3.300",
    ] {
        assert!(parse_pool(bad).is_err(), "{bad}");
    }
    assert_eq!(
        default_pool(SERVER, Ipv4Addr::new(255, 255, 255, 0)),
        Some((Ipv4Addr::new(192, 168, 3, 100), Ipv4Addr::new(192, 168, 3, 199)))
    );
    assert_eq!(default_pool(SERVER, Ipv4Addr::new(255, 255, 0, 0)), None);
}
