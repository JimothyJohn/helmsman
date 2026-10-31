//! Site discovery (`--site`): the beacon format held exactly, the peer
//! list held to what a hostile sender on the LAN can do with it, two
//! boxes hearing each other over real sockets (unicast loopback, and the
//! multicast group with loopback on), and `GET /api/site` with its one
//! cross-origin allowance.

use std::io::{Read as _, Write as _};
use std::net::{IpAddr, SocketAddr, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use helmsman_plc::api::{
    self, Api, Catalog, Config, Devices, Extras, Limits, Live, MemAudit, MemProposals, Shared,
};
use helmsman_plc::site::{
    self, BadBeacon, Beacon, EXPIRY, Heard, INTAKE, MAX_BEACON, MAX_PEERS, MIN_GAP, Peers, Site,
};
use proptest::prelude::*;

const ID_A: &str = "0123456789abcdef";
const ID_B: &str = "fedcba9876543210";

fn beacon(site: &str, id: &str, name: &str, api: u16) -> Beacon {
    Beacon::new(site, id, name, api, "0.1.0").unwrap()
}

fn lan(last: u8) -> SocketAddr {
    SocketAddr::from(([192, 168, 1, last], site::PORT))
}

fn s(secs: f64) -> Duration {
    Duration::from_secs_f64(secs)
}

// ---- the beacon -------------------------------------------------------

#[test]
fn a_beacon_is_six_exact_lines() {
    let b = beacon("line-a", ID_A, "Press 4", 8080);
    let wire = b.encode();
    assert_eq!(
        String::from_utf8(wire.clone()).unwrap(),
        "helmsman-site 1\nsite line-a\nid 0123456789abcdef\nname Press 4\napi 8080\nversion 0.1.0\n"
    );
    assert_eq!(Beacon::parse(&wire), Ok(b));
}

#[test]
fn anything_but_exactly_a_beacon_is_dropped() {
    let good = "helmsman-site 1\nsite line-a\nid 0123456789abcdef\nname Press 4\napi 8080\nversion 0.1.0\n";
    assert!(Beacon::parse(good.as_bytes()).is_ok());
    let bad = [
        // Framing.
        good.trim_end_matches('\n').to_string(),
        good.replace('\n', "\r\n"),
        format!("{good}extra 1\n"),
        format!("{good}\n"),
        format!("\n{good}"),
        good.replace("helmsman-site 1", "helmsman-site 2"),
        good.replace("helmsman-site 1", "HELMSMAN-SITE 1"),
        // Order, duplicates, missing.
        good.replace(
            "site line-a\nid 0123456789abcdef",
            "id 0123456789abcdef\nsite line-a",
        ),
        good.replace("name Press 4", "site line-a"),
        good.replace("api 8080\n", ""),
        // Separators.
        good.replace("api 8080", "api  8080"),
        good.replace("api 8080", "api\t8080"),
        good.replace("api 8080", "api8080"),
        good.replace("name Press 4", "name: Press 4"),
        // Fields.
        good.replace("api 8080", "api 0"),
        good.replace("api 8080", "api 08080"),
        good.replace("api 8080", "api 65536"),
        good.replace("api 8080", "api +8080"),
        good.replace("api 8080", "api -1"),
        good.replace("api 8080", "api "),
        good.replace(ID_A, "0123456789ABCDEF"),
        good.replace(ID_A, "0123456789abcde"),
        good.replace(ID_A, "0123456789abcdef0"),
        good.replace(ID_A, "0123456789abcdeg"),
        good.replace("line-a", ""),
        good.replace("line-a", "line a"),
        good.replace("line-a", &"x".repeat(33)),
        good.replace("Press 4", " Press 4"),
        good.replace("Press 4", "Press 4 "),
        good.replace("Press 4", "Press\u{e9} 4"),
        good.replace("Press 4", "<script>alert(1)</script>"),
        good.replace("Press 4", "Press/4"),
        good.replace("Press 4", &"x".repeat(49)),
        good.replace("Press 4", ""),
        good.replace("0.1.0", "0.1.0 beta"),
        good.replace("Press 4", "Press\u{0} 4"),
    ];
    for b in &bad {
        assert_eq!(Beacon::parse(b.as_bytes()), Err(BadBeacon::Malformed), "{b:?}");
    }
    // Not UTF-8 at all.
    let mut raw = good.as_bytes().to_vec();
    raw[30] = 0xFF;
    assert_eq!(Beacon::parse(&raw), Err(BadBeacon::Malformed));
}

#[test]
fn an_oversized_datagram_is_dropped_before_it_is_read() {
    assert_eq!(
        Beacon::parse(&vec![b'a'; MAX_BEACON + 1]),
        Err(BadBeacon::Oversized)
    );
    // The largest valid beacon fits with room to spare.
    let big = Beacon::new(
        &"s".repeat(32),
        ID_A,
        &"n".repeat(site::MAX_NAME),
        65_535,
        &"v".repeat(32),
    )
    .unwrap();
    assert!(big.encode().len() <= MAX_BEACON);
    assert_eq!(Beacon::parse(&big.encode()), Ok(big));
}

#[test]
fn a_box_id_is_stable_hex_and_says_nothing_of_the_token() {
    let token = "ab".repeat(32);
    let id = site::box_id(&token);
    assert!(site::valid_id(&id), "{id}");
    assert_eq!(id, site::box_id(&token));
    assert_ne!(id, site::box_id(&"cd".repeat(32)));
    assert!(!token.contains(&id));
}

fn token_strategy(max: usize) -> impl Strategy<Value = String> {
    proptest::string::string_regex(&format!("[A-Za-z0-9._-]{{1,{max}}}")).unwrap()
}

fn name_strategy() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[A-Za-z0-9._-]([A-Za-z0-9 ._-]{0,46}[A-Za-z0-9._-])?").unwrap()
}

proptest! {
    #[test]
    fn every_valid_beacon_round_trips(
        site in token_strategy(32),
        id in "[0-9a-f]{16}",
        name in name_strategy(),
        api in 1u16..,
        version in token_strategy(32),
    ) {
        let b = Beacon::new(&site, &id, &name, api, &version).unwrap();
        prop_assert_eq!(Beacon::parse(&b.encode()), Ok(b));
    }

    #[test]
    fn parsing_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..1200)) {
        let _ = Beacon::parse(&bytes);
    }

    #[test]
    fn a_damaged_beacon_never_panics_and_what_parses_is_valid(
        at in 0usize..200,
        byte in any::<u8>(),
        cut in 0usize..200,
    ) {
        let mut wire = beacon("line-a", ID_A, "Press 4", 8080).encode();
        let at = at % wire.len();
        wire[at] = byte;
        wire.truncate(wire.len() - cut % wire.len());
        if let Ok(b) = Beacon::parse(&wire) {
            prop_assert!(site::valid_token(&b.site) && site::valid_id(&b.id));
            prop_assert!(site::valid_name(&b.name) && site::valid_token(&b.version) && b.api > 0);
            prop_assert_eq!(Beacon::parse(&b.encode()), Ok(b));
        }
    }

    #[test]
    fn any_datagram_from_anywhere_leaves_the_list_bounded(
        grams in proptest::collection::vec(
            (proptest::collection::vec(any::<u8>(), 0..600), any::<[u8; 4]>(), 0u64..120_000),
            0..300,
        ),
    ) {
        let mut peers = Peers::new("line-a", ID_A);
        let mut t = 0u64;
        for (bytes, ip, dt) in grams {
            t += dt;
            peers.datagram(&bytes, SocketAddr::from((ip, 1)), Duration::from_millis(t));
            prop_assert!(peers.list(Duration::from_millis(t)).len() <= MAX_PEERS);
        }
    }
}

// ---- the peer list ----------------------------------------------------

#[test]
fn a_peer_is_listed_at_the_packets_address_and_forgotten_after_expiry() {
    let mut peers = Peers::new("line-a", ID_A);
    let b = beacon("line-a", ID_B, "Press 5", 9090);
    assert_eq!(peers.datagram(&b.encode(), lan(7), s(1.0)), Heard::New);
    let list = peers.list(s(1.0));
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].addr, SocketAddr::from(([192, 168, 1, 7], 9090)));
    assert_eq!(
        (list[0].name.as_str(), list[0].version.as_str()),
        ("Press 5", "0.1.0")
    );
    // Heard again: refreshed (a rename shows).
    let renamed = beacon("line-a", ID_B, "Press 5b", 9090);
    assert_eq!(
        peers.datagram(&renamed.encode(), lan(7), s(6.0)),
        Heard::Refreshed
    );
    assert_eq!(peers.list(s(6.0))[0].name, "Press 5b");
    assert_eq!(peers.list(s(6.0))[0].first, s(1.0));
    // Silent: still listed just before expiry, gone at it.
    let gone = s(6.0) + EXPIRY;
    assert_eq!(
        peers
            .list(gone.checked_sub(Duration::from_millis(1)).unwrap())
            .len(),
        1
    );
    assert_eq!(peers.list(gone), Vec::new());
    // Back again: a new peer.
    assert_eq!(peers.datagram(&b.encode(), lan(7), gone), Heard::New);
}

#[test]
fn own_other_sites_and_non_local_beacons_are_ignored() {
    let mut peers = Peers::new("line-a", ID_A);
    let own = beacon("line-a", ID_A, "Me", 8080);
    assert_eq!(peers.datagram(&own.encode(), lan(2), s(1.0)), Heard::Own);
    let other = beacon("line-b", ID_B, "Elsewhere", 8080);
    assert_eq!(peers.datagram(&other.encode(), lan(3), s(1.0)), Heard::OtherSite);
    let mine = beacon("line-a", ID_B, "Far", 8080).encode();
    for from in [
        SocketAddr::from(([8, 8, 8, 8], 1)),
        SocketAddr::from(([0, 0, 0, 0], 1)),
        SocketAddr::from(([255, 255, 255, 255], 1)),
        SocketAddr::from(([224, 0, 0, 1], 1)),
        SocketAddr::from(([100, 128, 0, 1], 1)),
        "[::1]:1".parse().unwrap(),
        "[fe80::1]:1".parse().unwrap(),
    ] {
        assert_eq!(peers.datagram(&mine, from, s(1.0)), Heard::NotLocal, "{from}");
    }
    for ip in [
        [10, 0, 0, 1],
        [172, 16, 0, 1],
        [169, 254, 1, 1],
        [127, 0, 0, 1],
        [100, 64, 0, 1],
    ] {
        assert!(site::local_source(IpAddr::from(ip)), "{ip:?}");
    }
    assert_eq!(peers.list(s(1.0)), Vec::new());
    let c = peers.counts();
    assert_eq!((c.own, c.other_site, c.not_local, c.accepted), (1, 1, 7, 0));
}

#[test]
fn a_spoofed_id_from_another_address_cannot_take_a_peer_over() {
    let mut peers = Peers::new("line-a", ID_A);
    let real = beacon("line-a", ID_B, "Press 5", 8080);
    assert_eq!(peers.datagram(&real.encode(), lan(7), s(1.0)), Heard::New);
    let spoof = beacon("line-a", ID_B, "Press 5", 6666);
    assert_eq!(peers.datagram(&spoof.encode(), lan(66), s(3.0)), Heard::Conflict);
    let p = &peers.list(s(3.0))[0];
    assert_eq!(p.addr, SocketAddr::from(([192, 168, 1, 7], 8080)));
    assert_eq!(p.conflict, Some(IpAddr::from([192, 168, 1, 66])));
    // The spoofer does not keep it alive either: the real box's silence
    // still expires it.
    for k in 0..27 {
        assert_eq!(
            peers.datagram(&spoof.encode(), lan(66), s(4.0 + f64::from(k))),
            Heard::Conflict
        );
    }
    assert_eq!(peers.list(s(1.0) + EXPIRY), Vec::new());
    assert_eq!(peers.counts().conflict, 28);
    // Only once the real one is gone may the id be heard elsewhere.
    assert_eq!(
        peers.datagram(&spoof.encode(), lan(66), s(1.0) + EXPIRY),
        Heard::New
    );
}

#[test]
fn a_peer_beaconing_too_fast_is_refreshed_at_most_once_per_gap() {
    let mut peers = Peers::new("line-a", ID_A);
    let b = beacon("line-a", ID_B, "Chatty", 8080).encode();
    assert_eq!(peers.datagram(&b, lan(7), s(10.0)), Heard::New);
    assert_eq!(peers.datagram(&b, lan(7), s(10.5)), Heard::TooSoon);
    assert_eq!(peers.datagram(&b, lan(7), s(10.0) + MIN_GAP), Heard::Refreshed);
    assert_eq!(peers.list(s(12.0))[0].last, s(10.0) + MIN_GAP);
    assert_eq!(peers.counts().too_soon, 1);
}

fn nth_id(k: usize) -> String {
    format!("{k:016x}")
}

#[test]
fn a_flood_of_new_ids_fills_the_list_but_never_displaces_it() {
    let mut peers = Peers::new("line-a", "ffffffffffffffff");
    // Spread over seconds so the intake limit is not what stops them.
    let at = |k: usize| Duration::from_millis(50 * u64::try_from(k).unwrap());
    for k in 0..MAX_PEERS {
        let b = beacon("line-a", &nth_id(k), "Box", 8080);
        assert_eq!(peers.datagram(&b.encode(), lan(9), at(k)), Heard::New);
    }
    let more = beacon("line-a", &nth_id(MAX_PEERS), "Late", 8080).encode();
    assert_eq!(peers.datagram(&more, lan(9), s(4.0)), Heard::Full);
    assert_eq!(peers.list(s(4.0)).len(), MAX_PEERS);
    assert!(peers.list(s(4.0)).iter().all(|p| p.name == "Box"));
    // Once they expire, there is room.
    assert_eq!(peers.datagram(&more, lan(9), at(MAX_PEERS) + EXPIRY), Heard::New);
    assert_eq!(peers.counts().full, 1);
}

#[test]
fn past_the_intake_a_second_datagrams_are_dropped_unread() {
    let mut peers = Peers::new("line-a", ID_A);
    let junk = b"not a beacon";
    for _ in 0..INTAKE {
        assert_eq!(peers.datagram(junk, lan(9), s(5.1)), Heard::Malformed);
    }
    let good = beacon("line-a", ID_B, "Late", 8080).encode();
    assert_eq!(peers.datagram(&good, lan(7), s(5.9)), Heard::Throttled);
    assert_eq!(peers.list(s(5.9)), Vec::new());
    // The next second takes datagrams again.
    assert_eq!(peers.datagram(&good, lan(7), s(6.0)), Heard::New);
    let c = peers.counts();
    assert_eq!((c.malformed, c.throttled, c.accepted), (u64::from(INTAKE), 1, 1));
}

// ---- over sockets -----------------------------------------------------

fn quiet() -> site::Log {
    Arc::new(|_: &str| {})
}

fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn two_boxes_hear_each_other_over_loopback() {
    let sa = UdpSocket::bind("127.0.0.1:0").unwrap();
    let sb = UdpSocket::bind("127.0.0.1:0").unwrap();
    let (pa, pb) = (sa.local_addr().unwrap(), sb.local_addr().unwrap());
    let every = Duration::from_millis(50);
    let a = Site::start(sa, beacon("line-a", ID_A, "Press 4", 8081), pb, every, quiet()).unwrap();
    let b = Site::start(sb, beacon("line-a", ID_B, "Press 5", 8082), pa, every, quiet()).unwrap();
    wait_for("each to hear the other", || {
        a.handle().snapshot().0.len() == 1 && b.handle().snapshot().0.len() == 1
    });
    let (heard_by_a, ..) = a.handle().snapshot();
    assert_eq!(heard_by_a[0].id, ID_B);
    assert_eq!(heard_by_a[0].addr, "127.0.0.1:8082".parse().unwrap());
    let (heard_by_b, ..) = b.handle().snapshot();
    assert_eq!(heard_by_b[0].addr, "127.0.0.1:8081".parse().unwrap());
    // A stranger's junk and an oversized datagram reach a listener and
    // change nothing but the counts; nothing is sent back.
    let stranger = UdpSocket::bind("127.0.0.1:0").unwrap();
    stranger.send_to(b"helmsman-site 1\n", pa).unwrap();
    stranger.send_to(&[b'x'; 2000], pa).unwrap();
    wait_for("the junk to be counted", || {
        let c = a.handle().snapshot().1;
        c.malformed >= 1 && c.oversized >= 1
    });
    stranger
        .set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    let mut buf = [0u8; 64];
    assert!(stranger.recv_from(&mut buf).is_err(), "a box answered a datagram");
    assert_eq!(a.handle().snapshot().0.len(), 1);
}

/// Linux only (the box's OS, and CI's): on macOS a freshly built,
/// unsigned test binary is not delivered multicast (the same sockets work
/// from the system Python), so there this would only test the Mac.
#[cfg(target_os = "linux")]
#[test]
fn two_boxes_meet_on_the_multicast_group() {
    // Two listeners on one host cannot share a port without SO_REUSEADDR
    // (std cannot set it), so box A holds the group port and box B sends
    // to the group from a port of its own: A hears B by multicast, with
    // loopback on, as boxes on one switch would.
    let probe = UdpSocket::bind("0.0.0.0:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let group = SocketAddr::from((site::GROUP, port));
    let sa = site::bind(SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, port)), group).unwrap();
    let sb = site::bind("0.0.0.0:0".parse().unwrap(), group).unwrap();
    let every = Duration::from_millis(50);
    let a = Site::start(sa, beacon("line-a", ID_A, "Press 4", 8081), group, every, quiet()).unwrap();
    let _b = Site::start(sb, beacon("line-a", ID_B, "Press 5", 8082), group, every, quiet()).unwrap();
    wait_for("A to hear B on the group", || {
        a.handle().snapshot().0.iter().any(|p| p.id == ID_B)
    });
    let (peers, counts, _) = a.handle().snapshot();
    assert_eq!(peers.len(), 1, "{peers:?}");
    assert_eq!(peers[0].addr.port(), 8082);
    // A's own beacons looped back and were ignored.
    wait_for("A's own beacon to loop back", || a.handle().snapshot().1.own > 0);
    assert_eq!(counts.malformed, 0);
}

// ---- the API ----------------------------------------------------------

fn tmp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "site-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const INDEX: &str = "<!doctype html><html><head><script>1</script></head><body></body></html>";

fn api(site: Option<site::Handle>) -> Api {
    let ui = tmp("ui");
    std::fs::write(ui.join("index.html"), INDEX).unwrap();
    let devices = Devices::new(["drive".to_string(), "io".to_string(), "scale".to_string()]);
    devices.set("drive", None);
    devices.set("io", Some("timed out".into()));
    let cfg = Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        token: "0".repeat(64),
        limits: Limits::default(),
        stream_every: Duration::from_millis(10),
        memory: 0,
        log: Arc::new(|_: &str| {}),
    };
    let shared = Shared {
        catalog: Catalog::default(),
        live: Arc::new(Live::default()),
        devices: Arc::new(devices),
        audit: Box::new(MemAudit::new(10)),
        proposals: Box::new(MemProposals::new(10)),
        running: None,
        snapshots: None,
        approval: None,
        chat: None,
    };
    let extras = Extras {
        ui: Some(api::ui::Ui::open(&ui).unwrap()),
        ..Extras::default()
    };
    Api::start_in_site(cfg, shared, extras, site).unwrap()
}

struct Resp {
    status: u16,
    head: String,
    body: String,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.head.split("\r\n").skip(1).find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case(name).then(|| v.trim())
        })
    }
}

fn send(addr: SocketAddr, raw: &str) -> Resp {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    let (head, body) = out.split_once("\r\n\r\n").unwrap();
    Resp {
        status: head[9..12].parse().unwrap(),
        head: head.to_string(),
        body: body.to_string(),
    }
}

fn get(addr: SocketAddr, path: &str) -> Resp {
    send(
        addr,
        &format!("GET {path} HTTP/1.1\r\nHost: box\r\nOrigin: http://10.0.0.9:8080\r\n\r\n"),
    )
}

#[test]
fn without_a_site_there_is_no_site_route_and_no_cross_origin_anything() {
    let a = api(None);
    let r = get(a.addr(), "/api/site");
    assert_eq!(r.status, 404);
    assert!(r.body.contains("--site"), "{}", r.body);
    assert_eq!(r.header("Access-Control-Allow-Origin"), None);
    let page = get(a.addr(), "/");
    assert_eq!(page.status, 200);
    let csp = page.header("Content-Security-Policy").unwrap();
    assert!(csp.contains("connect-src 'self';"), "{csp}");
}

#[test]
fn the_site_route_lists_peers_and_only_it_answers_other_origins() {
    let sa = UdpSocket::bind("127.0.0.1:0").unwrap();
    let sb = UdpSocket::bind("127.0.0.1:0").unwrap();
    let (pa, pb) = (sa.local_addr().unwrap(), sb.local_addr().unwrap());
    let every = Duration::from_millis(50);
    let me = Site::start(sa, beacon("line-a", ID_A, "Press 4", 8081), pb, every, quiet()).unwrap();
    let _peer = Site::start(sb, beacon("line-a", ID_B, "Press 5", 8082), pa, every, quiet()).unwrap();
    wait_for("the peer", || me.handle().snapshot().0.len() == 1);
    let a = api(Some(me.handle()));

    let r = get(a.addr(), "/api/site");
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.header("Access-Control-Allow-Origin"), Some("*"));
    assert_eq!(r.header("Access-Control-Allow-Credentials"), None);
    let j: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(j["site"], "line-a");
    assert_eq!(j["self"]["id"], ID_A);
    assert_eq!(j["self"]["name"], "Press 4");
    assert_eq!(j["self"]["api_port"], 8081);
    assert_eq!(j["self"]["status"]["modules"], 0);
    assert_eq!(
        j["self"]["status"]["devices"],
        serde_json::json!({ "up": 1, "down": 1, "unknown": 1 })
    );
    // Counts only: no device names, no errors, no values cross origins.
    for secret in ["drive", "timed out", "scale"] {
        assert!(!r.body.contains(secret), "{secret} in {}", r.body);
    }
    let peers = j["peers"].as_array().unwrap();
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0]["id"], ID_B);
    assert_eq!(peers[0]["name"], "Press 5");
    assert_eq!(peers[0]["address"], "127.0.0.1:8082");
    assert_eq!(peers[0]["conflict"], serde_json::Value::Null);
    assert!(peers[0]["last_seen_ms"].as_u64().unwrap() < 10_000);
    assert_eq!(j["max_peers"], MAX_PEERS);
    assert!(j["dropped"]["own"].is_u64(), "{}", j["dropped"]);

    // Every other read stays same-origin.
    for path in [
        "/api",
        "/api/signals",
        "/api/modules",
        "/api/devices",
        "/api/audit",
        "/api/nope",
    ] {
        assert_eq!(
            get(a.addr(), path).header("Access-Control-Allow-Origin"),
            None,
            "{path}"
        );
    }
    // No preflight: a cross-origin write could never be allowed.
    let pre = send(
        a.addr(),
        "OPTIONS /api/site HTTP/1.1\r\nHost: box\r\nOrigin: http://evil\r\nAccess-Control-Request-Method: POST\r\n\r\n",
    );
    assert_eq!(pre.status, 401);
    assert_eq!(pre.header("Access-Control-Allow-Origin"), None);
    let post = send(
        a.addr(),
        "POST /api/site HTTP/1.1\r\nHost: box\r\nContent-Length: 0\r\n\r\n",
    );
    assert_eq!(post.status, 401);
    // The canvas may fetch from peers; its scripts are still its own.
    let page = get(a.addr(), "/");
    let csp = page.header("Content-Security-Policy").unwrap();
    assert!(csp.contains("connect-src 'self' http:;"), "{csp}");
    assert!(
        csp.contains("script-src 'self' 'wasm-unsafe-eval' 'sha256-"),
        "{csp}"
    );
    assert!(csp.contains("default-src 'none'"), "{csp}");
    // The index lists the route.
    assert!(get(a.addr(), "/api").body.contains("/api/site"));
}

// ---- the flags --------------------------------------------------------

fn plc_err(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_helmsman-plc"))
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("programs/echo.st"))
        .arg("--state-dir")
        .arg(tmp("state"))
        .args(["--opcua", "off"])
        .args(args)
        .output()
        .unwrap();
    assert!(!out.status.success(), "{args:?} started");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn the_site_flags_are_checked_before_anything_starts() {
    assert!(plc_err(&["--site", "line a"]).contains("--site"));
    assert!(plc_err(&["--site", &"x".repeat(33)]).contains("--site"));
    assert!(plc_err(&["--site", "line-a", "--api", "off"]).contains("fixed port"));
    assert!(plc_err(&["--site", "line-a", "--api", "127.0.0.1:0"]).contains("fixed port"));
    assert!(plc_err(&["--box-name", "Press 4"]).contains("needs --site"));
    assert!(plc_err(&["--site", "line-a", "--box-name", " Press"]).contains("--box-name"));
}
