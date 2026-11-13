//! The server's contract, through bytes: a client's exchange from DISCOVER
//! to RELEASE, each REQUEST form of RFC 2131 4.3.2, BOOTP, the pool's
//! limits, and the lease file across a restart.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use helmsman_dhcp::message::{BOOTREPLY, BOOTREQUEST, FLAG_BROADCAST, Kind, opt};
use helmsman_dhcp::server::{Binding, ConfigError, DECLINE_HOLD_S, Event, FOREVER, OFFER_HOLD_S, State};
use helmsman_dhcp::{Config, Dest, Mac, Message, Server, leases};

const SERVER: Ipv4Addr = Ipv4Addr::new(192, 168, 3, 1);
const OTHER: Ipv4Addr = Ipv4Addr::new(192, 168, 3, 254);
const T0: u64 = 1_763_000_000;

fn ip(last: u8) -> Ipv4Addr {
    Ipv4Addr::new(192, 168, 3, last)
}

fn mac(n: u16) -> Mac {
    let [a, b] = n.to_be_bytes();
    Mac([0x00, 0x07, 0x46, 0x00, a, b])
}

fn config(first: u8, last: u8) -> Config {
    Config {
        server: SERVER,
        netmask: Ipv4Addr::new(255, 255, 255, 0),
        router: None,
        pool: (ip(first), ip(last)),
        lease_s: 3600,
        reserved: BTreeMap::new(),
        bootp: true,
    }
}

fn server(first: u8, last: u8) -> Server {
    Server::new(config(first, last)).unwrap()
}

fn request(mac: Mac, kind: Option<Kind>) -> Message {
    let mut m = Message::empty(BOOTREQUEST);
    m.xid = u32::from(mac.0[5]) | 0x1234_0000;
    m.chaddr[..6].copy_from_slice(&mac.0);
    if let Some(k) = kind {
        m.set_option(opt::MESSAGE_TYPE, &[k.code()]);
    }
    m
}

/// Through the wire both ways, as the box will see it.
fn send(s: &mut Server, now: u64, m: &Message) -> Option<(Dest, Message)> {
    let req = Message::parse(&m.encode()).unwrap();
    s.handle(now, &req)
        .map(|r| (r.dest, Message::parse(&r.message.encode()).unwrap()))
}

fn discover(s: &mut Server, now: u64, mac: Mac) -> Option<Message> {
    send(s, now, &request(mac, Some(Kind::Discover))).map(|(_, m)| m)
}

fn select(s: &mut Server, now: u64, mac: Mac, server_id: Ipv4Addr, want: Ipv4Addr) -> Option<Message> {
    let mut r = request(mac, Some(Kind::Request));
    r.set_option(opt::SERVER_ID, &server_id.octets());
    r.set_option(opt::REQUESTED_IP, &want.octets());
    send(s, now, &r).map(|(_, m)| m)
}

/// DISCOVER, OFFER, REQUEST, ACK: the address `mac` ends up with.
fn lease(s: &mut Server, now: u64, mac: Mac) -> Ipv4Addr {
    let offer = discover(s, now, mac).expect("an offer");
    let ack = select(s, now, mac, SERVER, offer.yiaddr).expect("an answer");
    assert_eq!(ack.kind(), Some(Kind::Ack));
    ack.yiaddr
}

#[test]
fn a_client_out_of_the_box_gets_the_first_address_with_everything_rfc_2131_requires() {
    let mut s = server(100, 199);
    let (dest, offer) = send(&mut s, T0, &request(mac(1), Some(Kind::Discover))).unwrap();
    assert_eq!(offer.op, BOOTREPLY);
    assert_eq!(offer.kind(), Some(Kind::Offer));
    assert_eq!(offer.yiaddr, ip(100));
    assert_eq!(offer.xid, request(mac(1), None).xid);
    assert_eq!(offer.mac(), Some(mac(1)));
    assert_eq!(offer.option_ip(opt::SERVER_ID), Some(SERVER));
    assert_eq!(offer.option(opt::LEASE_TIME), Some(&3600u32.to_be_bytes()[..]));
    assert_eq!(offer.option(opt::RENEWAL_TIME), Some(&1800u32.to_be_bytes()[..]));
    assert_eq!(
        offer.option(opt::REBINDING_TIME),
        Some(&3150u32.to_be_bytes()[..])
    );
    assert_eq!(
        offer.option_ip(opt::SUBNET_MASK),
        Some(Ipv4Addr::new(255, 255, 255, 0))
    );
    assert_eq!(
        offer.option(opt::ROUTER),
        None,
        "a device network gets no gateway unless configured"
    );
    assert_eq!(offer.option(opt::REQUESTED_IP), None, "Table 3: MUST NOT");
    assert_eq!(
        dest,
        Dest::Unicast {
            ip: ip(100),
            mac: Some(mac(1))
        }
    );

    let ack = select(&mut s, T0 + 1, mac(1), SERVER, ip(100)).unwrap();
    assert_eq!((ack.kind(), ack.yiaddr), (Some(Kind::Ack), ip(100)));
    assert_eq!(
        s.bindings()[&mac(1)],
        Binding {
            ip: ip(100),
            state: State::Bound,
            until: T0 + 1 + 3600
        }
    );
    assert_eq!(
        s.take_events(),
        vec![Event::Offered(mac(1), ip(100)), Event::Leased(mac(1), ip(100))]
    );
}

#[test]
fn the_broadcast_flag_gets_a_broadcast_and_a_nak_is_always_broadcast() {
    let mut s = server(100, 199);
    let mut d = request(mac(1), Some(Kind::Discover));
    d.flags = FLAG_BROADCAST;
    let (dest, offer) = send(&mut s, T0, &d).unwrap();
    assert_eq!(dest, Dest::Broadcast);
    assert_eq!(offer.flags, FLAG_BROADCAST);

    let mut r = request(mac(1), Some(Kind::Request));
    r.set_option(opt::SERVER_ID, &SERVER.octets());
    r.set_option(opt::REQUESTED_IP, &ip(150).octets());
    let (dest, nak) = send(&mut s, T0, &r).unwrap();
    assert_eq!(
        (dest, nak.kind(), nak.yiaddr),
        (Dest::Broadcast, Some(Kind::Nak), Ipv4Addr::UNSPECIFIED)
    );
    assert_eq!(nak.option(opt::LEASE_TIME), None, "Table 3: MUST NOT");
}

#[test]
fn a_device_keeps_its_address_across_power_cycles_and_lease_ends() {
    let mut s = server(100, 199);
    assert_eq!(lease(&mut s, T0, mac(1)), ip(100));
    assert_eq!(lease(&mut s, T0, mac(2)), ip(101));
    // Long after both leases ran out, in the other order.
    let later = T0 + 10 * 3600;
    assert_eq!(lease(&mut s, later, mac(2)), ip(101));
    assert_eq!(lease(&mut s, later, mac(1)), ip(100));
}

#[test]
fn a_discover_asking_for_a_free_address_gets_it_and_one_held_by_another_does_not() {
    let mut s = server(100, 199);
    lease(&mut s, T0, mac(1));
    let mut d = request(mac(2), Some(Kind::Discover));
    d.set_option(opt::REQUESTED_IP, &ip(150).octets());
    assert_eq!(send(&mut s, T0, &d).unwrap().1.yiaddr, ip(150));
    let mut d = request(mac(3), Some(Kind::Discover));
    d.set_option(opt::REQUESTED_IP, &ip(100).octets());
    assert_eq!(send(&mut s, T0, &d).unwrap().1.yiaddr, ip(101));
    let mut d = request(mac(4), Some(Kind::Discover));
    d.set_option(opt::REQUESTED_IP, &ip(20).octets());
    assert_eq!(
        send(&mut s, T0, &d).unwrap().1.yiaddr,
        ip(102),
        "outside the pool is not handed out"
    );
}

#[test]
fn a_reservation_wins_and_its_address_goes_to_nobody_else() {
    let mut c = config(100, 101);
    c.reserved.insert(mac(9), ip(100));
    c.reserved.insert(mac(8), ip(20));
    let mut s = Server::new(c).unwrap();
    assert_eq!(lease(&mut s, T0, mac(1)), ip(101));
    assert_eq!(
        discover(&mut s, T0, mac(2)),
        None,
        "the pool's other address is reserved"
    );
    assert_eq!(s.take_events().last(), Some(&Event::PoolFull(mac(2))));
    assert_eq!(lease(&mut s, T0, mac(9)), ip(100));
    assert_eq!(
        lease(&mut s, T0, mac(8)),
        ip(20),
        "a reservation may sit outside the pool"
    );
}

#[test]
fn selecting_another_server_frees_our_offer_and_gets_no_answer() {
    let mut s = server(100, 100);
    assert_eq!(discover(&mut s, T0, mac(1)).unwrap().yiaddr, ip(100));
    assert_eq!(discover(&mut s, T0, mac(2)), None);
    assert_eq!(select(&mut s, T0, mac(1), OTHER, ip(50)), None);
    assert_eq!(discover(&mut s, T0, mac(2)).unwrap().yiaddr, ip(100));
}

#[test]
fn an_offer_nobody_takes_comes_back_after_its_hold() {
    let mut s = server(100, 100);
    discover(&mut s, T0, mac(1)).unwrap();
    assert_eq!(discover(&mut s, T0 + OFFER_HOLD_S - 1, mac(2)), None);
    assert_eq!(
        discover(&mut s, T0 + OFFER_HOLD_S, mac(2)).unwrap().yiaddr,
        ip(100)
    );
    // mac(1)'s REQUEST now arrives too late: NAK, and mac(2) keeps it.
    assert_eq!(
        select(&mut s, T0 + OFFER_HOLD_S, mac(1), SERVER, ip(100))
            .unwrap()
            .kind(),
        Some(Kind::Nak)
    );
    assert_eq!(
        select(&mut s, T0 + OFFER_HOLD_S, mac(2), SERVER, ip(100))
            .unwrap()
            .kind(),
        Some(Kind::Ack)
    );
}

#[test]
fn init_reboot_acks_its_own_address_naks_a_wrong_one_and_ignores_strangers() {
    let mut s = server(100, 199);
    let got = lease(&mut s, T0, mac(1));
    let reboot = |m: Mac, want: Ipv4Addr| {
        let mut r = request(m, Some(Kind::Request));
        r.set_option(opt::REQUESTED_IP, &want.octets());
        r
    };
    assert_eq!(
        send(&mut s, T0 + 5, &reboot(mac(1), got)).unwrap().1.kind(),
        Some(Kind::Ack)
    );
    assert_eq!(
        send(&mut s, T0 + 5, &reboot(mac(1), ip(150))).unwrap().1.kind(),
        Some(Kind::Nak)
    );
    assert_eq!(
        send(&mut s, T0 + 5, &reboot(mac(1), Ipv4Addr::new(10, 0, 0, 57)))
            .unwrap()
            .1
            .kind(),
        Some(Kind::Nak),
        "on the wrong network"
    );
    assert_eq!(
        send(&mut s, T0 + 5, &reboot(mac(2), ip(150))),
        None,
        "no record: silent (4.3.2)"
    );
}

#[test]
fn renewing_extends_by_unicast_and_a_renewal_after_a_lost_file_is_taken_back_on() {
    let mut s = server(100, 199);
    let got = lease(&mut s, T0, mac(1));
    let mut r = request(mac(1), Some(Kind::Request));
    r.ciaddr = got;
    let (dest, ack) = send(&mut s, T0 + 1800, &r).unwrap();
    assert_eq!(
        (dest, ack.kind(), ack.yiaddr),
        (Dest::Unicast { ip: got, mac: None }, Some(Kind::Ack), got)
    );
    assert_eq!(s.bindings()[&mac(1)].until, T0 + 1800 + 3600);

    // The box lost its lease file; the device renews what it had.
    let mut fresh = server(100, 199);
    assert_eq!(send(&mut fresh, T0, &r).unwrap().1.kind(), Some(Kind::Ack));
    // ...unless another device holds it by now, or it is not ours to give.
    let mut taken = server(100, 199);
    lease(&mut taken, T0, mac(2));
    let mut r2 = request(mac(1), Some(Kind::Request));
    r2.ciaddr = ip(100);
    assert_eq!(send(&mut taken, T0, &r2), None);
    let mut outside = request(mac(3), Some(Kind::Request));
    outside.ciaddr = ip(20);
    assert_eq!(send(&mut taken, T0, &outside), None);
}

#[test]
fn a_declined_address_is_skipped_until_its_hold_ends() {
    let mut s = server(100, 101);
    let got = lease(&mut s, T0, mac(1));
    let mut d = request(mac(1), Some(Kind::Decline));
    d.set_option(opt::SERVER_ID, &SERVER.octets());
    d.set_option(opt::REQUESTED_IP, &got.octets());
    assert_eq!(send(&mut s, T0, &d), None);
    assert_eq!(s.take_events().last(), Some(&Event::Declined(mac(1), got)));
    assert_eq!(discover(&mut s, T0 + 1, mac(1)).unwrap().yiaddr, ip(101));
    assert_eq!(discover(&mut s, T0 + 2, mac(2)), None);
    assert_eq!(
        discover(&mut s, T0 + DECLINE_HOLD_S, mac(2)).unwrap().yiaddr,
        ip(100)
    );
}

#[test]
fn a_decline_or_release_for_another_server_changes_nothing() {
    let mut s = server(100, 199);
    let got = lease(&mut s, T0, mac(1));
    let before = s.bindings().clone();
    for kind in [Kind::Decline, Kind::Release] {
        let mut m = request(mac(1), Some(kind));
        m.ciaddr = got;
        m.set_option(opt::SERVER_ID, &OTHER.octets());
        m.set_option(opt::REQUESTED_IP, &got.octets());
        assert_eq!(send(&mut s, T0, &m), None);
    }
    assert_eq!(s.bindings(), &before);
}

#[test]
fn a_released_address_stays_its_owners_first_choice_until_the_pool_needs_it() {
    let mut s = server(100, 101);
    let got = lease(&mut s, T0, mac(1));
    let mut r = request(mac(1), Some(Kind::Release));
    r.ciaddr = got;
    r.set_option(opt::SERVER_ID, &SERVER.octets());
    assert_eq!(send(&mut s, T0, &r), None);
    assert_eq!(s.bindings()[&mac(1)].state, State::Released);
    assert_eq!(lease(&mut s, T0, mac(2)), ip(101), "the never-used address first");
    assert_eq!(lease(&mut s, T0, mac(3)), ip(100), "then the released one");
    assert_eq!(discover(&mut s, T0, mac(1)), None);
}

#[test]
fn a_flood_of_made_up_clients_fills_the_pool_but_moves_no_device() {
    let mut s = server(100, 109);
    let devices: Vec<Ipv4Addr> = (0..3).map(|n| lease(&mut s, T0, mac(n))).collect();
    for n in 1000..6000u16 {
        let _ = discover(&mut s, T0 + 1, mac(n));
    }
    assert!(s.bindings().len() <= 10 + 3);
    for (n, want) in (0..3).zip(&devices) {
        // Renewal and reboot both still answer, with the same address.
        let mut r = request(mac(n), Some(Kind::Request));
        r.ciaddr = *want;
        assert_eq!(send(&mut s, T0 + 2, &r).unwrap().1.yiaddr, *want);
        assert_eq!(lease(&mut s, T0 + 3, mac(n)), *want);
    }
    // A real new device gets in once the flood's offers lapse.
    assert_eq!(discover(&mut s, T0 + 2, mac(500)), None);
    assert!(discover(&mut s, T0 + 2 + OFFER_HOLD_S, mac(500)).is_some());
}

#[test]
fn an_expired_lease_is_taken_back_only_when_nothing_else_is_left() {
    let mut s = server(100, 101);
    lease(&mut s, T0, mac(1));
    lease(&mut s, T0 + 100, mac(2));
    let later = T0 + 100 + 3600;
    // Both expired; the older one goes.
    assert_eq!(lease(&mut s, later, mac(3)), ip(100));
    assert!(!s.bindings().contains_key(&mac(1)));
    assert_eq!(s.bindings()[&mac(2)].ip, ip(101));
}

#[test]
fn bootp_gets_a_padded_reply_and_a_binding_that_never_expires() {
    let mut s = server(100, 199);
    let mut b = request(mac(1), None);
    b.cookie = false;
    let wire = b.encode();
    let req = Message::parse(&wire).unwrap();
    let reply = s.handle(T0, &req).unwrap();
    let bytes = reply.message.encode();
    assert_eq!(bytes.len(), 300, "RFC 1542 2.1");
    let back = Message::parse(&bytes).unwrap();
    assert_eq!(
        (back.op, back.yiaddr, back.siaddr, back.cookie),
        (BOOTREPLY, ip(100), SERVER, false)
    );
    assert_eq!(
        s.bindings()[&mac(1)],
        Binding {
            ip: ip(100),
            state: State::Bound,
            until: FOREVER
        }
    );
    // With vendor extensions: the mask, no message type.
    let mut b2 = request(mac(2), None);
    b2.cookie = true;
    let back = send(&mut s, T0 + 10 * 3600, &b2).unwrap().1;
    assert_eq!(
        (back.kind(), back.option_ip(opt::SUBNET_MASK)),
        (None, Some(Ipv4Addr::new(255, 255, 255, 0)))
    );
    // Asked again a year later: the same address.
    assert_eq!(send(&mut s, T0 + 365 * 86_400, &b).unwrap().1.yiaddr, ip(100));

    let mut off = config(100, 199);
    off.bootp = false;
    assert_eq!(Server::new(off).unwrap().handle(T0, &req), None);
}

#[test]
fn relayed_requests_replies_inform_and_non_ethernet_get_nothing() {
    let mut s = server(100, 199);
    let mut relayed = request(mac(1), Some(Kind::Discover));
    relayed.giaddr = ip(2);
    let mut reply = request(mac(1), Some(Kind::Discover));
    reply.op = BOOTREPLY;
    let mut token_ring = request(mac(1), Some(Kind::Discover));
    token_ring.htype = 6;
    let mut long_hw = request(mac(1), Some(Kind::Discover));
    long_hw.hlen = 16;
    let mut unknown_type = request(mac(1), None);
    unknown_type.set_option(opt::MESSAGE_TYPE, &[200]);
    for m in [
        relayed,
        reply,
        token_ring,
        long_hw,
        request(mac(1), Some(Kind::Inform)),
        unknown_type,
    ] {
        assert_eq!(send(&mut s, T0, &m), None, "{m:?}");
    }
    assert!(s.bindings().is_empty());
}

#[test]
fn a_bad_configuration_is_refused_by_name() {
    let mut c = config(100, 199);
    c.netmask = Ipv4Addr::new(255, 0, 255, 0);
    assert_eq!(Server::new(c).unwrap_err(), ConfigError::Netmask);
    assert_eq!(Server::new(config(199, 100)).unwrap_err(), ConfigError::PoolOrder);
    assert_eq!(
        Server::new(config(1, 10)).unwrap_err(),
        ConfigError::PoolHoldsServer
    );
    assert_eq!(
        Server::new(config(100, 255)).unwrap_err(),
        ConfigError::OffNetwork(ip(255))
    );
    assert_eq!(
        Server::new(config(0, 10)).unwrap_err(),
        ConfigError::OffNetwork(ip(0))
    );
    let mut c = config(100, 199);
    c.pool.1 = Ipv4Addr::new(192, 168, 4, 10);
    assert!(matches!(Server::new(c).unwrap_err(), ConfigError::OffNetwork(_)));
    let mut c = config(100, 199);
    c.reserved.insert(mac(1), ip(50));
    c.reserved.insert(mac(2), ip(50));
    assert_eq!(
        Server::new(c).unwrap_err(),
        ConfigError::DoubleReservation(ip(50))
    );
    let mut c = config(100, 199);
    c.reserved.insert(mac(1), SERVER);
    assert_eq!(
        Server::new(c).unwrap_err(),
        ConfigError::DoubleReservation(SERVER)
    );
    let mut c = config(100, 199);
    c.lease_s = 0;
    assert_eq!(Server::new(c).unwrap_err(), ConfigError::LeaseZero);
}

#[test]
fn leases_survive_a_restart_through_the_file() {
    let mut s = server(100, 199);
    let a = lease(&mut s, T0, mac(1));
    let mut b = request(mac(2), None);
    b.cookie = false;
    send(&mut s, T0, &b).unwrap();
    discover(&mut s, T0, mac(3)).unwrap();
    let text = leases::write(s.bindings());

    let mut again = server(100, 199);
    assert_eq!(again.restore(leases::parse(&text).unwrap()), 0);
    assert_eq!(again.bindings().len(), 2, "the open offer is not kept");
    assert_eq!(lease(&mut again, T0 + 7200, mac(1)), a);
    assert_eq!(discover(&mut again, T0 + 7200, mac(4)).unwrap().yiaddr, ip(102));
}

#[test]
fn restore_skips_what_the_new_configuration_cannot_hold() {
    let mut c = config(100, 199);
    c.reserved.insert(mac(9), ip(105));
    let mut s = Server::new(c).unwrap();
    let b = |ip, state| Binding {
        ip,
        state,
        until: T0 + 10,
    };
    let skipped = s.restore([
        (mac(1), b(ip(100), State::Bound)),
        (mac(2), b(ip(100), State::Bound)),
        (mac(3), b(Ipv4Addr::new(10, 0, 0, 5), State::Bound)),
        (mac(4), b(SERVER, State::Bound)),
        (mac(5), b(ip(105), State::Bound)),
        (mac(6), b(ip(106), State::Offered)),
        (mac(9), b(ip(105), State::Bound)),
    ]);
    assert_eq!(skipped, 5);
    assert_eq!(
        s.bindings().keys().copied().collect::<Vec<_>>(),
        vec![mac(1), mac(9)]
    );
}

#[test]
fn forget_puts_an_address_back() {
    let mut s = server(100, 100);
    lease(&mut s, T0, mac(1));
    assert_eq!(discover(&mut s, T0, mac(2)), None);
    assert_eq!(s.forget(mac(1)).map(|b| b.ip), Some(ip(100)));
    assert_eq!(discover(&mut s, T0, mac(2)).unwrap().yiaddr, ip(100));
}

#[test]
fn taking_back_a_lapsed_offer_never_hands_out_another_clients_reservation() {
    // Found by tests/hostile.rs: the take-back checked the reservation
    // against the lapsed binding's owner, not the client asking.
    let mut c = config(100, 101);
    c.reserved.insert(mac(9), ip(100));
    let mut s = Server::new(c).unwrap();
    assert_eq!(discover(&mut s, T0, mac(9)).unwrap().yiaddr, ip(100));
    assert_eq!(lease(&mut s, T0, mac(1)), ip(101));
    assert_eq!(discover(&mut s, T0 + OFFER_HOLD_S, mac(2)), None);
    assert_eq!(lease(&mut s, T0 + OFFER_HOLD_S, mac(9)), ip(100));
}
