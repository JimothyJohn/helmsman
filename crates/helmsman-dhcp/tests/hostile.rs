//! Whatever arrives, nothing panics, and no sequence of requests breaks
//! the server's invariants: one address per client, no address held by
//! two, nothing outside the network or on the server's own address, and
//! a lease once given is not taken away while it runs.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use helmsman_dhcp::message::{BOOTREQUEST, COOKIE, FIXED_LEN, Kind, opt};
use helmsman_dhcp::server::State;
use helmsman_dhcp::{Config, Mac, Message, Server, guard, leases};
use proptest::prelude::*;

const SERVER: Ipv4Addr = Ipv4Addr::new(192, 168, 3, 1);
const T0: u64 = 1_763_000_000;

fn server() -> Server {
    let mut reserved = BTreeMap::new();
    reserved.insert(Mac([0, 7, 0x46, 0, 0, 3]), Ipv4Addr::new(192, 168, 3, 102));
    Server::new(Config {
        server: SERVER,
        netmask: Ipv4Addr::new(255, 255, 255, 0),
        router: Some(SERVER),
        pool: (Ipv4Addr::new(192, 168, 3, 100), Ipv4Addr::new(192, 168, 3, 105)),
        lease_s: 120,
        reserved,
        bootp: true,
    })
    .unwrap()
}

/// A request a client (or someone pretending) might send: a few MACs so
/// they collide, every message type and then some, addresses near and
/// inside the pool, the server's id or another's.
fn arb_request() -> impl Strategy<Value = Message> {
    (
        0u8..8,
        prop_oneof![Just(None), (0u8..10).prop_map(Some), Just(Some(200u8))],
        proptest::option::of(90u8..110),
        proptest::option::of(prop_oneof![Just(1u8), Just(254u8)]),
        prop_oneof![Just(0u8), 95u8..108],
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(|(m, kind, want, server_id, ciaddr, cookie, broadcast)| {
            let mut r = Message::empty(BOOTREQUEST);
            r.chaddr[..6].copy_from_slice(&[0, 7, 0x46, 0, 0, m]);
            r.cookie = cookie || kind.is_some();
            r.flags = if broadcast { 0x8000 } else { 0 };
            if ciaddr != 0 {
                r.ciaddr = Ipv4Addr::new(192, 168, 3, ciaddr);
            }
            if let Some(k) = kind {
                r.set_option(opt::MESSAGE_TYPE, &[k]);
            }
            if let Some(w) = want {
                r.set_option(opt::REQUESTED_IP, &[192, 168, 3, w]);
            }
            if let Some(id) = server_id {
                r.set_option(opt::SERVER_ID, &[192, 168, 3, id]);
            }
            r
        })
}

fn check_invariants(s: &Server, now: u64, given: &BTreeMap<Mac, (Ipv4Addr, u64)>) {
    let mut held: Vec<Ipv4Addr> = Vec::new();
    for (mac, b) in s.bindings() {
        let o = b.ip.octets();
        assert_eq!(&o[..3], &[192, 168, 3], "{mac}: {}", b.ip);
        assert!(b.ip != SERVER && o[3] != 0 && o[3] != 255, "{mac}: {}", b.ip);
        if b.holds(now) {
            assert!(!held.contains(&b.ip), "{} held twice", b.ip);
            held.push(b.ip);
        }
    }
    // A lease handed out in an ACK is still that client's while it runs.
    for (mac, (ip, until)) in given {
        if *until > now {
            let b = s
                .bindings()
                .get(mac)
                .unwrap_or_else(|| panic!("{mac} lost its running lease"));
            assert_eq!(b.ip, *ip, "{mac} moved off its running lease");
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn any_bytes_decode_or_fail_and_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..700)) {
        if let Ok(m) = Message::parse(&bytes) {
            // What decodes, encodes and decodes to the same message.
            let again = Message::parse(&m.encode()).unwrap();
            prop_assert_eq!(&again, &m);
            let _ = server().handle(T0, &m);
            let _ = guard::foreign(&m, SERVER, Some(SERVER));
        }
    }

    #[test]
    fn options_of_any_shape_never_panic(opts in proptest::collection::vec(any::<u8>(), 0..400), file in proptest::collection::vec(any::<u8>(), 128), sname in proptest::collection::vec(any::<u8>(), 64)) {
        let mut bytes = Message::empty(BOOTREQUEST).encode();
        bytes.truncate(FIXED_LEN);
        bytes[44..108].copy_from_slice(&sname);
        bytes[108..236].copy_from_slice(&file);
        bytes.extend_from_slice(&COOKIE);
        bytes.extend_from_slice(&opts);
        if let Ok(m) = Message::parse(&bytes) {
            let mut s = server();
            let _ = s.handle(T0, &m);
        }
    }

    #[test]
    fn lease_text_of_any_shape_never_panics(text in "\\PC{0,400}") {
        let _ = leases::parse(&text);
        let _ = leases::parse(&format!("{}\n{text}", leases::HEADER));
    }

    #[test]
    fn no_request_sequence_breaks_the_invariants(steps in proptest::collection::vec((arb_request(), 0u64..90), 1..120)) {
        let mut s = server();
        let mut now = T0;
        let mut given: BTreeMap<Mac, (Ipv4Addr, u64)> = BTreeMap::new();
        for (req, dt) in steps {
            now += dt;
            let req = Message::parse(&req.encode()).unwrap();
            if let Some(reply) = s.handle(now, &req) {
                let m = &reply.message;
                prop_assert_eq!(m.xid, req.xid);
                prop_assert_eq!(m.option_ip(opt::SERVER_ID).unwrap_or(SERVER), SERVER);
                if m.kind() == Some(Kind::Ack) || (!req.is_dhcp() && m.kind().is_none()) {
                    let mac = req.mac().unwrap();
                    let b = s.bindings()[&mac];
                    prop_assert_eq!(b.state, State::Bound);
                    prop_assert_eq!(b.ip, m.yiaddr);
                    given.insert(mac, (b.ip, b.until));
                }
            }
            // A release or a decline gives a lease back by the client's
            // own choice.
            if matches!(req.kind(), Some(Kind::Release | Kind::Decline)) {
                given.retain(|mac, _| s.bindings().get(mac).is_some_and(|b| b.state == State::Bound));
            }
            check_invariants(&s, now, &given);
            // And the file always reads back to what it wrote.
            let text = leases::write(s.bindings());
            let mut again = server();
            prop_assert_eq!(again.restore(leases::parse(&text).unwrap()), 0);
        }
    }
}
