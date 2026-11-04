//! Read Device Identification (FC 43/14, card `modbus-identification`):
//! the client against the simulator and against hostile servers over real
//! sockets, the reply parser under proptest, and the ARP-table probe that
//! resolves a `modbus=VENDOR:PRODUCT` endpoint.

use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use helmsman_io::Error;
use helmsman_io::modbus::{self, Client, DeviceIdentification, MAX_ID_REPLIES};
use helmsman_io::resolve::{self, Endpoint, Identity, ModbusProbe, Resolver};
use helmsman_io::sim::{Identification, Sim, State};
use proptest::prelude::*;

fn ident_sim(id: Identification) -> Sim {
    let mut st = State::new(8);
    st.identification = Some(id);
    Sim::start(st).unwrap()
}

fn moxa() -> Identification {
    Identification::basic("Moxa", "E1213", "V3.1")
}

#[test]
fn the_client_reads_the_simulators_identification() {
    let s = ident_sim(moxa());
    let mut c = Client::new(&s.endpoint(), 1, Duration::from_millis(500));
    let id = c.read_device_identification().unwrap();
    assert_eq!(
        id,
        DeviceIdentification {
            vendor_name: "Moxa".into(),
            product_code: "E1213".into(),
            revision: "V3.1".into(),
            conformity: 0x81,
        }
    );
    // The one-shot probe says the same, and nothing on the device moved.
    let again = modbus::identify(s.addr, 1, Duration::from_millis(500)).unwrap();
    assert_eq!(again, id);
    assert_eq!(s.state().coils, vec![false; 8]);
    assert_eq!(s.state().holding, vec![0; 8]);
}

#[test]
fn more_follows_is_followed_to_the_end() {
    // One object a reply: three request/response transactions (App §6.21 p.45).
    let mut id = moxa();
    id.per_reply = 1;
    id.objects.push((0x04, b"ioLogik E1213".to_vec()));
    let s = ident_sim(id);
    let got = modbus::identify(s.addr, 1, Duration::from_secs(1)).unwrap();
    assert_eq!(got.vendor_name, "Moxa");
    assert_eq!(got.revision, "V3.1");
    assert_eq!(s.state().requests, 4, "one request per object");
}

#[test]
fn the_simulator_answers_as_the_spec_says() {
    let s = ident_sim(moxa());
    let mut c = Client::new(&s.endpoint(), 1, Duration::from_millis(500));
    // Individual access (code 04): one object; an unknown one is 02.
    let one = c.transact(&[0x2B, 0x0E, 0x04, 0x01]).unwrap();
    assert_eq!(
        one,
        [
            0x2B, 0x0E, 0x04, 0x81, 0x00, 0x00, 0x01, 0x01, 5, b'E', b'1', b'2', b'1', b'3'
        ]
    );
    assert_eq!(c.transact(&[0x2B, 0x0E, 0x04, 0x09]).unwrap(), [0xAB, 0x02]);
    // An illegal Read Device ID code is 03; an unknown MEI type is 01.
    assert_eq!(c.transact(&[0x2B, 0x0E, 0x05, 0x00]).unwrap(), [0xAB, 0x03]);
    assert_eq!(c.transact(&[0x2B, 0x0D, 0x01, 0x00]).unwrap(), [0xAB, 0x01]);
    // A stream from an unknown object restarts at the first (p.44).
    let r = c.transact(&[0x2B, 0x0E, 0x01, 0x77]).unwrap();
    assert_eq!(r[7], 0x00, "{r:?}");
    // A device without the function says Illegal Function.
    let plain = Sim::start(State::new(8)).unwrap();
    assert!(matches!(
        modbus::identify(plain.addr, 1, Duration::from_millis(500)),
        Err(Error::Exception(modbus::ILLEGAL_FUNCTION))
    ));
}

/// A Modbus TCP server that frames whatever `answer` returns for each
/// request PDU (`None`: say nothing, keep the connection). Returns its
/// address and how many connections it took.
fn scripted(
    answer: impl Fn(usize, &[u8]) -> Option<Vec<u8>> + Send + Sync + 'static,
) -> (SocketAddr, Arc<AtomicUsize>) {
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let addr = l.local_addr().unwrap();
    let conns = Arc::new(AtomicUsize::new(0));
    let (c2, answer) = (Arc::clone(&conns), Arc::new(answer));
    thread::spawn(move || {
        for conn in l.incoming() {
            let Ok(mut conn) = conn else { return };
            c2.fetch_add(1, Ordering::SeqCst);
            let answer = Arc::clone(&answer);
            thread::spawn(move || {
                let mut k = 0;
                loop {
                    let mut head = [0u8; 7];
                    if conn.read_exact(&mut head).is_err() {
                        return;
                    }
                    let len = usize::from(u16::from_be_bytes([head[4], head[5]]));
                    let mut pdu = vec![0u8; len.saturating_sub(1)];
                    if conn.read_exact(&mut pdu).is_err() {
                        return;
                    }
                    if let Some(body) = answer(k, &pdu) {
                        let mut f = head[..4].to_vec();
                        f.extend(u16::try_from(body.len() + 1).unwrap_or(u16::MAX).to_be_bytes());
                        f.push(head[6]);
                        f.extend(body);
                        if conn.write_all(&f).is_err() {
                            return;
                        }
                    }
                    k += 1;
                }
            });
        }
    });
    (addr, conns)
}

/// A basic-identification reply body with these fields and objects.
fn page(more: u8, next: u8, count: u8, objects: &[(u8, &[u8])]) -> Vec<u8> {
    let mut r = vec![0x2B, 0x0E, 0x01, 0x01, more, next, count];
    for (id, v) in objects {
        r.push(*id);
        r.push(u8::try_from(v.len()).unwrap());
        r.extend(*v);
    }
    r
}

const BASIC: [(u8, &[u8]); 3] = [(0, b"Moxa"), (1, b"E1213"), (2, b"V3.1")];

fn ask(addr: SocketAddr) -> (Result<DeviceIdentification, Error>, Duration) {
    let t = Instant::now();
    let r = modbus::identify(addr, 1, Duration::from_millis(400));
    (r, t.elapsed())
}

/// What a scripted server answers the k-th request with.
type Answer = Box<dyn Fn(usize, &[u8]) -> Option<Vec<u8>> + Send + Sync>;

#[test]
fn hostile_servers_get_a_bounded_error_never_a_hang() {
    for (what, answer) in hostile_cases() {
        let (addr, _) = scripted(answer);
        let (r, took) = ask(addr);
        assert!(r.is_err(), "{what}: {r:?}");
        assert!(took < Duration::from_secs(2), "{what}: took {took:?}");
    }
}

/// A server that lies, breaks the layout or never answers, each its own way.
fn hostile_cases() -> Vec<(&'static str, Answer)> {
    vec![
        (
            "wrong MEI type",
            Box::new(|_, _| {
                let mut p = page(0, 0, 3, &BASIC);
                p[1] = 0x0D;
                Some(p)
            }),
        ),
        (
            "wrong Read Device ID code",
            Box::new(|_, _| {
                let mut p = page(0, 0, 3, &BASIC);
                p[2] = 0x03;
                Some(p)
            }),
        ),
        (
            "count says more than sent",
            Box::new(|_, _| Some(page(0, 0, 9, &BASIC))),
        ),
        (
            "count says fewer than sent",
            Box::new(|_, _| Some(page(0, 0, 1, &BASIC))),
        ),
        (
            "truncated object",
            Box::new(|_, _| {
                let mut p = page(0, 0, 3, &BASIC);
                p.truncate(p.len() - 2);
                Some(p)
            }),
        ),
        (
            "oversize length",
            Box::new(|_, _| {
                let mut p = page(0, 0, 1, &[(0, b"Moxa")]);
                p[8] = 0xFF;
                Some(p)
            }),
        ),
        (
            "More Follows not 00 or FF",
            Box::new(|_, _| Some(page(0x01, 0, 3, &BASIC))),
        ),
        (
            "More Follows restarting at 0",
            Box::new(|_, _| Some(page(0xFF, 0, 1, &[(0, b"Moxa")]))),
        ),
        (
            "More Follows standing still",
            Box::new(|k, _| {
                Some(if k == 0 {
                    page(0xFF, 1, 1, &[(0, b"Moxa")])
                } else {
                    page(0xFF, 1, 0, &[])
                })
            }),
        ),
        (
            "More Follows forever",
            Box::new(|k, _| {
                let id = u8::try_from(k + 3).unwrap_or(u8::MAX);
                Some(if k == 0 {
                    page(0xFF, 3, 3, &BASIC)
                } else {
                    page(0xFF, id.saturating_add(1), 1, &[(id, b"x")])
                })
            }),
        ),
        (
            "an object twice",
            Box::new(|_, _| {
                Some(page(
                    0,
                    0,
                    4,
                    &[(0, b"Moxa"), (1, b"E1213"), (2, b"V3.1"), (0, b"Evil")],
                ))
            }),
        ),
        (
            "no ProductCode",
            Box::new(|_, _| Some(page(0, 0, 2, &[(0, b"Moxa"), (2, b"V3.1")]))),
        ),
        ("Illegal Function", Box::new(|_, _| Some(vec![0xAB, 0x01]))),
        ("Server Device Failure", Box::new(|_, _| Some(vec![0xAB, 0x04]))),
        (
            "malformed exception",
            Box::new(|_, _| Some(vec![0xAB, 0x01, 0x02])),
        ),
        (
            "another function",
            Box::new(|_, _| Some(vec![0x03, 0x02, 0x00, 0x00])),
        ),
        ("empty PDU", Box::new(|_, _| Some(Vec::new()))),
        ("never answers", Box::new(|_, _| None)),
    ]
}

#[test]
fn a_more_follows_chain_that_never_ends_is_cut_at_the_cap() {
    let asked = Arc::new(AtomicUsize::new(0));
    let a2 = Arc::clone(&asked);
    let (addr, _) = scripted(move |k, _| {
        a2.fetch_add(1, Ordering::SeqCst);
        let id = u8::try_from(k + 3).unwrap_or(u8::MAX);
        Some(if k == 0 {
            page(0xFF, 3, 3, &BASIC)
        } else {
            page(0xFF, id.saturating_add(1), 1, &[(id, b"x")])
        })
    });
    let e = modbus::identify(addr, 1, Duration::from_secs(2)).unwrap_err();
    assert!(e.to_string().contains("More Follows"), "{e}");
    assert_eq!(asked.load(Ordering::SeqCst), MAX_ID_REPLIES);
}

#[test]
fn names_that_are_not_utf8_or_carry_control_bytes_are_cleaned_not_fatal() {
    let (addr, _) = scripted(|_, _| {
        Some(page(
            0,
            0,
            3,
            &[
                (0, b"Mo\xFFxa\x1b[31m"),
                (1, b"E1213\r\nINJECTED"),
                (2, b"  \0V3.1\0 "),
            ],
        ))
    });
    let id = modbus::identify(addr, 1, Duration::from_millis(500)).unwrap();
    assert_eq!(id.vendor_name, "Mo\u{FFFD}xa[31m");
    assert_eq!(id.product_code, "E1213INJECTED");
    assert_eq!(id.revision, "V3.1");
    // A long name is cut, so a page or a log line stays bounded.
    let long = vec![b'A'; 200];
    let (addr, _) = scripted(move |_, _| Some(page(0, 0, 3, &[(0, &long), (1, b"P"), (2, b"R")])));
    let id = modbus::identify(addr, 1, Duration::from_millis(500)).unwrap();
    assert_eq!(id.vendor_name.chars().count(), modbus::MAX_ID_TEXT);
}

#[test]
fn a_trickling_server_cannot_stretch_the_deadline() {
    // One byte every 40 ms of a valid reply: a per-read timeout never
    // fires, the probe's deadline does.
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let addr = l.local_addr().unwrap();
    thread::spawn(move || {
        let Ok((mut c, _)) = l.accept() else { return };
        let mut head = [0u8; 11];
        if c.read_exact(&mut head).is_err() {
            return;
        }
        let body = page(0, 0, 3, &BASIC);
        let mut f = head[..4].to_vec();
        f.extend(u16::try_from(body.len() + 1).unwrap().to_be_bytes());
        f.push(head[6]);
        f.extend(body);
        for b in f {
            if c.write_all(&[b]).is_err() {
                return;
            }
            thread::sleep(Duration::from_millis(40));
        }
    });
    let t = Instant::now();
    let r = modbus::identify(addr, 1, Duration::from_millis(300));
    assert!(
        matches!(&r, Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::TimedOut),
        "{r:?}"
    );
    assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
}

#[test]
fn a_server_that_hangs_up_mid_reply_is_an_error() {
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let addr = l.local_addr().unwrap();
    thread::spawn(move || {
        let Ok((mut c, _)) = l.accept() else { return };
        let mut head = [0u8; 11];
        let _ = c.read_exact(&mut head);
        let _ = c.write_all(&[head[0], head[1], 0, 0, 0, 20, 1, 0x2B, 0x0E]);
    });
    assert!(modbus::identify(addr, 1, Duration::from_millis(500)).is_err());
}

fn arp_for(ips: &[Ipv4Addr]) -> String {
    let mut out = String::new();
    for (k, ip) in ips.iter().enumerate() {
        let _ = writeln!(out, "? ({ip}) at 2:0:0:0:0:{k:x} on en0 ifscope [ethernet]");
    }
    out
}

fn resolver(table: String, port: u16) -> Resolver {
    Resolver {
        arp: Box::new(move || Ok(table.clone())),
        sweep: None,
        window: Duration::from_millis(10),
        modbus: Some(ModbusProbe {
            port,
            per_host: Duration::from_millis(400),
            total: Duration::from_secs(2),
            ..ModbusProbe::default()
        }),
    }
}

#[test]
fn a_modbus_identity_resolves_through_the_probe() {
    let s = ident_sim(Identification::basic("Moxa Inc.", "E1213", "V3.1"));
    let r = resolver(arp_for(&[Ipv4Addr::LOCALHOST]), s.addr.port());
    let seen = resolve::probe_modbus(&r).unwrap();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].address, Ipv4Addr::LOCALHOST);
    assert_eq!(seen[0].identity().to_string(), "modbus=Moxa%20Inc.:E1213");
    assert_eq!(seen[0].mac_text().as_deref(), Some("mac=02-00-00-00-00-00"));
    // The endpoint a map gives: case-blind, and the address the driver's port.
    for endpoint in ["modbus=Moxa%20Inc.:E1213", "modbus=moxa%20inc.:e1213"] {
        assert_eq!(
            resolve::socket_addr(endpoint, 502, &r).unwrap(),
            "127.0.0.1:502".parse().unwrap(),
            "{endpoint}"
        );
    }
    let e = resolve::socket_addr("modbus=Moxa%20Inc.:E1214", 502, &r).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
    assert!(e.to_string().contains("among the 1 that answered"), "{e}");
    let off = Resolver {
        modbus: None,
        ..resolver(String::new(), 1)
    };
    let e = resolve::socket_addr("modbus=Moxa:E1213", 502, &off).unwrap_err();
    assert!(e.to_string().contains("no Modbus identification probe"), "{e}");
}

#[test]
fn the_probe_asks_only_hosts_the_arp_table_lists() {
    let (addr, conns) = scripted(|_, _| Some(page(0, 0, 3, &BASIC)));
    // 127.0.0.1 is not in the table: nothing reaches the server.
    let r = resolver(arp_for(&[Ipv4Addr::new(192, 0, 2, 1)]), addr.port());
    let mut r = r;
    if let Some(p) = r.modbus.as_mut() {
        p.per_host = Duration::from_millis(100);
    }
    let seen = resolve::probe_modbus(&r).unwrap();
    assert!(seen.is_empty(), "{seen:?}");
    assert_eq!(conns.load(Ordering::SeqCst), 0);
    // Listed: asked once.
    let r = resolver(arp_for(&[Ipv4Addr::LOCALHOST]), addr.port());
    assert_eq!(resolve::probe_modbus(&r).unwrap().len(), 1);
    assert_eq!(conns.load(Ordering::SeqCst), 1);
}

#[test]
fn the_probe_is_bounded_in_hosts_and_time() {
    // The table lists the box's peers only: incomplete entries, group MACs
    // and multicast or broadcast addresses are not hosts.
    let table = "IP address HW type Flags HW address Mask Device\n\
10.0.0.9 0x1 0x2 00:90:e8:7e:af:be * eth0\n\
10.0.0.99 0x1 0x0 00:00:00:00:00:00 * eth0\n\
? (10.0.0.3) at 0:1:2:3:4:5 on en1 ifscope [ethernet]\n\
? (10.0.0.3) at 0:1:2:3:4:5 on en1 ifscope [ethernet]\n\
? (10.0.0.60) at (incomplete) on en1 ifscope [ethernet]\n\
? (10.0.0.255) at ff:ff:ff:ff:ff:ff on en1 ifscope [ethernet]\n\
? (224.0.0.251) at 1:0:5e:0:0:fb on en1 ifscope permanent [ethernet]\n";
    let hosts: Vec<Ipv4Addr> = resolve::arp_hosts(table).into_iter().map(|(a, _)| a).collect();
    assert_eq!(hosts, [Ipv4Addr::new(10, 0, 0, 3), Ipv4Addr::new(10, 0, 0, 9)]);
    // A big table: only the first `max_hosts`, in address order, are asked.
    let many: Vec<Ipv4Addr> = (0..200u8).rev().map(|k| Ipv4Addr::new(192, 0, 2, k)).collect();
    let p = ModbusProbe {
        max_hosts: 64,
        ..ModbusProbe::default()
    };
    let asked = resolve::probe_targets(&arp_for(&many), &p);
    assert_eq!(asked.len(), 64);
    assert_eq!(asked[0].0, Ipv4Addr::new(192, 0, 2, 0));
    assert_eq!(asked[63].0, Ipv4Addr::new(192, 0, 2, 63));
    // Hosts that never answer (TEST-NET-1, RFC 5737: nothing is there):
    // the probe ends by its total plus one host's deadline, not after
    // 200 hosts / 8 at a time x 100 ms.
    let mut r = resolver(arp_for(&many), modbus::DEFAULT_PORT);
    if let Some(p) = r.modbus.as_mut() {
        p.max_hosts = 200;
        p.per_host = Duration::from_millis(100);
        p.total = Duration::from_millis(300);
    }
    let t = Instant::now();
    assert_eq!(resolve::probe_modbus(&r).unwrap(), Vec::new());
    assert!(t.elapsed() < Duration::from_millis(1500), "{:?}", t.elapsed());
    // A silent server that accepts and never answers: the same bound.
    let (addr, conns) = scripted(|_, _| None);
    let mut r = resolver(arp_for(&[Ipv4Addr::LOCALHOST]), addr.port());
    if let Some(p) = r.modbus.as_mut() {
        p.per_host = Duration::from_millis(200);
    }
    let t = Instant::now();
    assert_eq!(resolve::probe_modbus(&r).unwrap(), Vec::new());
    assert!(t.elapsed() < Duration::from_millis(1500), "{:?}", t.elapsed());
    assert_eq!(conns.load(Ordering::SeqCst), 1);
}

#[test]
fn modbus_identities_parse_strictly_and_print_back() {
    assert_eq!(
        resolve::parse("modbus=Moxa:E1213"),
        Ok(Endpoint::Identity(Identity::Modbus {
            vendor: "Moxa".into(),
            product: "E1213".into()
        }))
    );
    assert_eq!(
        resolve::parse("modbus=Schneider%20Electric:BMX%3AP34"),
        Ok(Endpoint::Identity(Identity::Modbus {
            vendor: "Schneider Electric".into(),
            product: "BMX:P34".into()
        }))
    );
    for bad in [
        "modbus=",
        "modbus=Moxa",
        "modbus=:E1213",
        "modbus=Moxa:",
        "modbus=Moxa:E1:213",
        "modbus=Mo xa:E1213",
        "modbus=Moxa%2:E1213",
        "modbus=Moxa%zz:E1213",
        "modbus=Moxa%FF:E1213",
        "modbus=Moxa%0A:E1213",
        "modbus=%20%20:E1213",
    ] {
        assert!(resolve::parse(bad).is_err(), "{bad:?}");
    }
    let long = format!("modbus={}:P", "a".repeat(modbus::MAX_ID_TEXT + 1));
    assert!(resolve::parse(&long).is_err());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Whatever bytes a reply holds, the parser returns a page or an error.
    #[test]
    fn the_reply_parser_never_panics(pdu in prop::collection::vec(any::<u8>(), 0..260), code in 1u8..=4) {
        let _ = helmsman_modbus::reply::device_id(code, &pdu);
    }

    /// Well-formed replies read back to exactly what was sent; one byte
    /// more or less never reads.
    #[test]
    fn well_formed_replies_round_trip(
        objects in prop::collection::vec((any::<u8>(), prop::collection::vec(any::<u8>(), 0..40)), 0..5),
        more in any::<bool>(),
        next in any::<u8>(),
        conformity in any::<u8>(),
    ) {
        let mut r = vec![0x2B, 0x0E, 0x01, conformity, if more { 0xFF } else { 0x00 }, next, u8::try_from(objects.len()).unwrap()];
        for (id, v) in &objects {
            r.push(*id);
            r.push(u8::try_from(v.len()).unwrap());
            r.extend(v);
        }
        let p = helmsman_modbus::reply::device_id(0x01, &r).unwrap();
        prop_assert_eq!(&p.objects, &objects);
        prop_assert_eq!(p.next, more.then_some(next));
        prop_assert_eq!(p.conformity, conformity);
        let mut longer = r.clone();
        longer.push(0);
        prop_assert!(helmsman_modbus::reply::device_id(0x01, &longer).is_err());
        if objects.iter().any(|(_, v)| !v.is_empty()) || !objects.is_empty() {
            prop_assert!(helmsman_modbus::reply::device_id(0x01, &r[..r.len() - 1]).is_err());
        }
    }

    /// The endpoint form round-trips any name a device may report.
    #[test]
    fn modbus_endpoints_round_trip(vendor in "\\PC{1,40}", product in "\\PC{1,40}") {
        prop_assume!(!vendor.trim().is_empty() && !product.trim().is_empty());
        let id = Identity::Modbus { vendor: vendor.clone(), product: product.clone() };
        let text = id.to_string();
        prop_assert!(!text.chars().any(char::is_whitespace) && !text.contains('#'), "{}", text);
        prop_assert_eq!(resolve::parse(&text), Ok(Endpoint::Identity(id)));
    }

    /// Whatever a server sends back, the probe ends within its budget.
    #[test]
    fn hostile_bytes_end_within_the_budget(junk in prop::collection::vec(any::<u8>(), 0..300), frame in any::<bool>()) {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = l.local_addr().unwrap();
        thread::spawn(move || {
            let Ok((mut c, _)) = l.accept() else { return };
            let mut head = [0u8; 11];
            if c.read_exact(&mut head).is_err() {
                return;
            }
            let mut out = Vec::new();
            if frame {
                out.extend(&head[..4]);
                out.extend(u16::try_from(junk.len() + 1).unwrap_or(u16::MAX).to_be_bytes());
                out.push(head[6]);
            }
            out.extend(&junk);
            let _ = c.write_all(&out);
            let mut sink = [0u8; 64];
            while matches!(c.read(&mut sink), Ok(n) if n > 0) {}
        });
        let t = Instant::now();
        let _ = modbus::identify(addr, 1, Duration::from_millis(150));
        prop_assert!(t.elapsed() < Duration::from_secs(1));
    }
}

#[test]
fn a_missing_modbus_device_is_not_probed_for_every_cycle() {
    let (addr, conns) = scripted(|_, _| Some(page(0, 0, 3, &[(0, b"Other"), (1, b"Thing"), (2, b"1")])));
    let r = resolver(arp_for(&[Ipv4Addr::LOCALHOST]), addr.port());
    let mut c = Client::new("modbus=Moxa:E1213", 1, Duration::from_millis(300)).with_resolver(r);
    let e = c.read_registers(0x03, 0, 1).unwrap_err();
    assert!(e.to_string().contains("no such device among the 1"), "{e}");
    assert_eq!(conns.load(Ordering::SeqCst), 1);
    // The link retries at once: the same answer, no second probe.
    let again = c.read_registers(0x03, 0, 1).unwrap_err();
    assert_eq!(again.to_string(), e.to_string());
    assert_eq!(conns.load(Ordering::SeqCst), 1);
    assert_eq!(c.resolved(), None);
}

#[test]
fn a_closed_port_fails_at_once() {
    // A closed port refuses at once rather than waiting out the budget.
    let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let t = Instant::now();
    assert!(matches!(
        modbus::identify(
            SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
            1,
            Duration::from_secs(1)
        ),
        Err(Error::Io(_))
    ));
    assert!(t.elapsed() < Duration::from_secs(1));
}
