//! The shared Class 1 I/O endpoint: the router's contract as a property,
//! then the socket on loopback with hostile traffic.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use helmsman_proto::endpoint::{IoEndpoint, OwnerId, Router, Verdict};
use helmsman_proto::io::IoPacket;
use proptest::prelude::*;

fn pkt(id: u32, seq: u32) -> Vec<u8> {
    IoPacket {
        connection_id: id,
        encapsulation_sequence: Some(seq),
        body: &[1, 0, 0xAA],
    }
    .encode()
}

#[derive(Debug, Clone)]
enum Op {
    Register { id: u32, owner: u32, peer: u8 },
    Unregister { id: u32, owner: u32 },
    DropOwner { owner: u32 },
    Sync { owner: u32, want: Vec<(u32, u8)> },
    Datagram { id: u32, from: u8, junk: bool },
}

fn ip(n: u8) -> Ipv4Addr {
    Ipv4Addr::new(10, 0, 0, n)
}

fn op() -> impl Strategy<Value = Op> {
    // Few IDs, owners and peers so ops collide often.
    let id = 0u32..6;
    let owner = 1u32..4;
    let peer = 1u8..4;
    prop_oneof![
        (id.clone(), owner.clone(), peer.clone()).prop_map(|(id, owner, peer)| Op::Register {
            id,
            owner,
            peer
        }),
        (id.clone(), owner.clone()).prop_map(|(id, owner)| Op::Unregister { id, owner }),
        owner.clone().prop_map(|owner| Op::DropOwner { owner }),
        (owner, proptest::collection::vec((id.clone(), peer.clone()), 0..4))
            .prop_map(|(owner, want)| Op::Sync { owner, want }),
        (id, peer, any::<bool>()).prop_map(|(id, from, junk)| Op::Datagram { id, from, junk }),
    ]
}

proptest! {
    /// Any sequence of registrations and datagrams: every delivered
    /// datagram went to the owner of its ID and came from that route's
    /// peer; everything else was dropped with the right reason.
    #[test]
    fn every_delivery_goes_to_the_owner_of_its_id_from_its_peer(ops in proptest::collection::vec(op(), 0..80)) {
        let mut r = Router::new(1);
        // The model: id -> (owner, peer), first registration wins.
        let mut model: HashMap<u32, (u32, u8)> = HashMap::new();
        for op in ops {
            match op {
                Op::Register { id, owner, peer } => {
                    let res = r.register(id, OwnerId(owner), ip(peer));
                    if let Some(&(o, _)) = model.get(&id) {
                        prop_assert_eq!(res.map_err(|t| t.owner), Err(OwnerId(o)));
                    } else {
                        prop_assert!(res.is_ok());
                        model.insert(id, (owner, peer));
                    }
                }
                Op::Unregister { id, owner } => {
                    let mine = model.get(&id).is_some_and(|&(o, _)| o == owner);
                    prop_assert_eq!(r.unregister(id, OwnerId(owner)), mine);
                    if mine {
                        model.remove(&id);
                    }
                }
                Op::DropOwner { owner } => {
                    r.unregister_all(OwnerId(owner));
                    model.retain(|_, (o, _)| *o != owner);
                }
                Op::Sync { owner, want } => {
                    let taken = r.sync(OwnerId(owner), &want.iter().map(|&(i, p)| (i, ip(p))).collect::<Vec<_>>());
                    model.retain(|id, (o, _)| *o != owner || want.iter().any(|(w, _)| w == id));
                    for &(id, peer) in &want {
                        match model.get(&id).copied() {
                            Some((o, _)) if o == owner => { model.insert(id, (owner, peer)); }
                            Some((o, _)) => prop_assert!(taken.iter().any(|t| t.connection_id == id && t.owner == OwnerId(o))),
                            None => { model.insert(id, (owner, peer)); }
                        }
                    }
                }
                Op::Datagram { id, from, junk } => {
                    let bytes = if junk { pkt(id, 1)[..5].to_vec() } else { pkt(id, 1) };
                    let v = r.route(&bytes, ip(from));
                    let want = if junk {
                        Verdict::Malformed
                    } else {
                        match model.get(&id) {
                            None => Verdict::Unknown(id),
                            Some(&(o, p)) if p != from => Verdict::WrongPeer { connection_id: id, owner: OwnerId(o) },
                            Some(&(o, _)) => Verdict::Deliver { owner: OwnerId(o), connection_id: id },
                        }
                    };
                    prop_assert_eq!(v, want);
                }
            }
            prop_assert_eq!(r.len(), model.len());
        }
    }

    /// Arbitrary bytes from anywhere never reach an owner unless they are a
    /// well-formed datagram for a registered ID from its peer.
    #[test]
    fn junk_is_never_delivered(bytes in proptest::collection::vec(any::<u8>(), 0..64), from in 1u8..4) {
        let mut r = Router::new(1);
        r.register(7, OwnerId(1), ip(1)).unwrap();
        if let Verdict::Deliver { owner, connection_id } = r.route(&bytes, ip(from)) {
            prop_assert_eq!(owner, OwnerId(1));
            prop_assert_eq!(connection_id, 7);
            prop_assert_eq!(from, 1);
            prop_assert_eq!(IoPacket::decode(&bytes).unwrap().connection_id, 7);
        }
    }

    #[test]
    fn allocated_ids_are_unique_and_never_registered(seed in any::<u32>(), taken in proptest::collection::vec(any::<u32>(), 0..20)) {
        let mut r = Router::new(seed);
        for t in &taken {
            let _ = r.register(*t, OwnerId(9), ip(1));
        }
        let mut seen = std::collections::HashSet::new();
        for _ in 0..50 {
            let id = r.allocate();
            prop_assert!(id != 0);
            prop_assert!(r.get(id).is_none());
            prop_assert!(seen.insert(id));
        }
    }
}

fn endpoint() -> IoEndpoint {
    IoEndpoint::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap()
}

fn wait(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn two_owners_share_one_socket_and_each_gets_only_its_own() {
    let ep = endpoint();
    let adapter = ep.owner();
    let scanner = ep.owner();
    let a = adapter.allocate();
    let s = scanner.allocate();
    assert_ne!(a, s);
    adapter.register(a, Ipv4Addr::LOCALHOST).unwrap();
    scanner.register(s, Ipv4Addr::LOCALHOST).unwrap();
    assert!(
        scanner.register(a, Ipv4Addr::LOCALHOST).is_err(),
        "an ID has one owner"
    );

    let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    for seq in 1..=5 {
        peer.send_to(&pkt(a, seq), ep.local_addr()).unwrap();
        peer.send_to(&pkt(s, seq), ep.local_addr()).unwrap();
        peer.send_to(&pkt(0xDEAD, seq), ep.local_addr()).unwrap();
    }
    let mut got_a = Vec::new();
    let mut got_s = Vec::new();
    wait("five each", || {
        while let Some(d) = adapter.recv_timeout(Duration::from_millis(1)) {
            got_a.push(d);
        }
        while let Some(d) = scanner.recv_timeout(Duration::from_millis(1)) {
            got_s.push(d);
        }
        got_a.len() == 5 && got_s.len() == 5
    });
    assert!(
        got_a
            .iter()
            .all(|d| d.connection_id == a && IoPacket::decode(&d.bytes).unwrap().connection_id == a)
    );
    assert!(got_s.iter().all(|d| d.connection_id == s));
    assert!(
        got_a
            .iter()
            .all(|d| d.from == SocketAddrV4::new(Ipv4Addr::LOCALHOST, peer.local_addr().unwrap().port()))
    );
    wait("unknown counted", || ep.stats().unknown == 5);

    // Replies leave from the shared port.
    adapter
        .send_to(
            &pkt(1, 1),
            match peer.local_addr().unwrap() {
                std::net::SocketAddr::V4(v) => v,
                std::net::SocketAddr::V6(_) => unreachable!(),
            },
        )
        .unwrap();
    let mut buf = [0u8; 64];
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let (_, from) = peer.recv_from(&mut buf).unwrap();
    assert_eq!(from.port(), ep.local_addr().port());
}

#[test]
fn a_datagram_with_another_owners_id_from_another_address_is_refused() {
    let ep = endpoint();
    let adapter = ep.owner();
    let scanner = ep.owner();
    let a = adapter.allocate();
    let s = scanner.allocate();
    // The adapter's scanner is 127.0.0.2; the device is 127.0.0.1.
    adapter.register(a, Ipv4Addr::new(127, 0, 0, 2)).unwrap();
    scanner.register(s, Ipv4Addr::LOCALHOST).unwrap();
    let device = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    // The device (or anyone at its address) uses the adapter's ID.
    for seq in 1..=10 {
        device.send_to(&pkt(a, seq), ep.local_addr()).unwrap();
    }
    device.send_to(&pkt(s, 1), ep.local_addr()).unwrap();
    let d = scanner
        .recv_timeout(Duration::from_secs(2))
        .expect("the scanner's own datagram");
    assert_eq!(d.connection_id, s);
    wait("ten refused", || ep.stats().wrong_peer == 10);
    assert!(adapter.recv_timeout(Duration::from_millis(20)).is_none());
    assert!(scanner.recv_timeout(Duration::from_millis(20)).is_none());
}

#[test]
fn dropping_an_owner_frees_its_ids_and_leaves_the_others() {
    let ep = endpoint();
    let keep = ep.owner();
    let k = keep.allocate();
    keep.register(k, Ipv4Addr::LOCALHOST).unwrap();
    let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    for round in 0..20u32 {
        let churn = ep.owner();
        let c = churn.allocate();
        churn.register(c, Ipv4Addr::LOCALHOST).unwrap();
        peer.send_to(&pkt(c, round + 1), ep.local_addr()).unwrap();
        peer.send_to(&pkt(k, round + 1), ep.local_addr()).unwrap();
        assert_eq!(
            churn.recv_timeout(Duration::from_secs(2)).unwrap().connection_id,
            c
        );
        assert_eq!(
            keep.recv_timeout(Duration::from_secs(2)).unwrap().connection_id,
            k
        );
        drop(churn);
        assert!(ep.route_of(c).is_none());
        assert!(ep.route_of(k).is_some());
    }
}

#[test]
fn a_full_queue_drops_its_oldest_and_never_the_other_owners() {
    let ep = endpoint();
    let flooded = ep.owner();
    let quiet = ep.owner();
    let f = flooded.allocate();
    let q = quiet.allocate();
    flooded.register(f, Ipv4Addr::LOCALHOST).unwrap();
    quiet.register(q, Ipv4Addr::LOCALHOST).unwrap();
    let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let n = u32::try_from(helmsman_proto::endpoint::QUEUE_LEN).unwrap() * 3;
    for seq in 1..=n {
        peer.send_to(&pkt(f, seq), ep.local_addr()).unwrap();
        if seq.is_multiple_of(64) {
            // Let the router keep up so the kernel buffer doesn't drop.
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    peer.send_to(&pkt(q, 1), ep.local_addr()).unwrap();
    assert_eq!(
        quiet.recv_timeout(Duration::from_secs(2)).unwrap().connection_id,
        q
    );
    wait("the flood routed", || ep.stats().delivered > u64::from(n) / 2);
    std::thread::sleep(Duration::from_millis(50));
    assert!(flooded.overflow() > 0);
    let mut last = 0;
    let mut count = 0;
    while let Some(d) = flooded.recv_timeout(Duration::from_millis(5)) {
        last = IoPacket::decode(&d.bytes)
            .unwrap()
            .encapsulation_sequence
            .unwrap();
        count += 1;
    }
    assert!(count <= helmsman_proto::endpoint::QUEUE_LEN);
    // Drop-oldest: what is left is the newest.
    assert!(last > n - 16, "newest kept, got {last} of {n}");
}
