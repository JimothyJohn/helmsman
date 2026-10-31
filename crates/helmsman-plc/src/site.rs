//! Site discovery (`--site NAME`, off by default): boxes on one LAN find
//! each other so one canvas can show the floor.
//!
//! Each box sends a small text **beacon** every [`EVERY`] to the
//! administratively scoped multicast group [`GROUP`]:[`PORT`] with a TTL
//! of 1 (it never leaves the subnet), and listens there for the others'.
//! A beacon says which site the box is in, its id, its name, the port of
//! its box API and its version:
//!
//! ```text
//! helmsman-site 1
//! site line-a
//! id 3f9c0a7d11e2b4c8
//! name Press 4
//! api 8080
//! version 0.1.0
//! ```
//!
//! **A beacon is untrusted input, and a box only lists peers.** Nothing a
//! beacon says makes the box do anything: it never answers one (no
//! reflection or amplification: the only datagram it sends is its own
//! beacon, on its own timer, to the fixed group), never connects to a peer,
//! never forwards what it heard. What it heard is served read-only at
//! `GET /api/site`, and the browser does the rest. So that list is held to
//! what a hostile sender on the LAN can do with it:
//!
//! - **Strict format** ([`Beacon::parse`]): at most [`MAX_BEACON`] bytes,
//!   ASCII, exactly the six lines above in that order, each field from a
//!   small alphabet with a length cap; anything else is dropped whole.
//! - **The address is the packet's, not the beacon's**: a peer's API is the
//!   datagram's source IP and the port the beacon names, so a beacon cannot
//!   point the list at a third host it did not send from (short of spoofing
//!   its source on the LAN itself). Sources outside the private, link-local
//!   and loopback ranges are dropped.
//! - **Rate limits**: at most [`INTAKE`] datagrams a second are looked at
//!   (the rest are counted and dropped unread); a known peer is refreshed
//!   at most once per [`MIN_GAP`].
//! - **Bounded**: at most [`MAX_PEERS`] peers; one not heard for
//!   [`EXPIRY`] is forgotten. When the list is full a new id waits until
//!   one expires (what is listed is never displaced by a flood).
//! - **No takeover**: an id heard from a second address while the first
//!   is still live does not move; the peer is marked as in conflict, which
//!   the site view shows.
//! - Other sites' beacons, and this box's own (multicast loops back), are
//!   ignored.
//!
//! Every drop is counted ([`Counts`]), and the counts are served with the
//! list, so a flood is visible rather than silent.

use std::collections::BTreeMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use sha2::{Digest as _, Sha256};

/// The first line of every beacon: the format and its version.
pub const MAGIC: &str = "helmsman-site 1";
/// The largest beacon, in bytes; a larger datagram is dropped unread.
pub const MAX_BEACON: usize = 512;
/// The multicast group beacons go to (239.255.0.0/16: organization-local
/// scope, never routed off the site).
pub const GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 72, 77);
/// The UDP port beacons go to and are heard on.
pub const PORT: u16 = 41_910;
/// How often a box sends its beacon.
pub const EVERY: Duration = Duration::from_secs(5);
/// A peer not heard for this long is forgotten (six beacons missed).
pub const EXPIRY: Duration = Duration::from_secs(30);
/// A known peer is refreshed at most this often.
pub const MIN_GAP: Duration = Duration::from_secs(1);
/// The most peers a box lists.
pub const MAX_PEERS: usize = 64;
/// Datagrams looked at per second; the rest are dropped unread.
pub const INTAKE: u32 = 100;

/// Longest site name and version, in bytes.
const MAX_TOKEN: usize = 32;
/// Longest box name, in bytes.
pub const MAX_NAME: usize = 48;
/// Hex digits in a box id.
pub const ID_LEN: usize = 16;

/// A site name or version: `[A-Za-z0-9._-]`, 1 to 32 bytes.
#[must_use]
pub fn valid_token(s: &str) -> bool {
    (1..=MAX_TOKEN).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A box name: `[A-Za-z0-9 ._-]`, 1 to 48 bytes, no space at either end.
#[must_use]
pub fn valid_name(s: &str) -> bool {
    (1..=MAX_NAME).contains(&s.len())
        && !s.starts_with(' ')
        && !s.ends_with(' ')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'.' | b'_' | b'-'))
}

/// A box id: 16 lower-case hex digits.
#[must_use]
pub fn valid_id(s: &str) -> bool {
    s.len() == ID_LEN && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A box's id, derived from its API token (one-way: SHA-256 with a label,
/// first 8 bytes), so it is stable across restarts and needs no file of
/// its own, and says nothing about the token.
#[must_use]
pub fn box_id(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"helmsman-site-id\0");
    h.update(token.as_bytes());
    h.finalize()[..ID_LEN / 2]
        .iter()
        .fold(String::with_capacity(ID_LEN), |mut s, b| {
            use std::fmt::Write as _;
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// What a box says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Beacon {
    pub site: String,
    pub id: String,
    pub name: String,
    /// The port of the box API.
    pub api: u16,
    pub version: String,
}

/// Why a datagram is not a beacon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadBeacon {
    /// Longer than [`MAX_BEACON`].
    Oversized,
    /// Anything else.
    Malformed,
}

/// A port as a beacon writes it: decimal, 1..=65535, no leading zero.
fn port(s: &str) -> Option<u16> {
    if s.is_empty() || s.len() > 5 || s.starts_with('0') || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok().filter(|p| *p != 0)
}

impl Beacon {
    /// A beacon, if every field is valid.
    ///
    /// # Errors
    /// The field that is not, and why.
    pub fn new(site: &str, id: &str, name: &str, api: u16, version: &str) -> Result<Self, String> {
        if !valid_token(site) {
            return Err(format!(
                "site name {site:?}: 1 to {MAX_TOKEN} of A-Z a-z 0-9 . _ -"
            ));
        }
        if !valid_id(id) {
            return Err(format!("box id {id:?}: {ID_LEN} lower-case hex digits"));
        }
        if !valid_name(name) {
            return Err(format!(
                "box name {name:?}: 1 to {MAX_NAME} of A-Z a-z 0-9 space . _ -, no space at either end"
            ));
        }
        if api == 0 {
            return Err("API port 0".into());
        }
        if !valid_token(version) {
            return Err(format!(
                "version {version:?}: 1 to {MAX_TOKEN} of A-Z a-z 0-9 . _ -"
            ));
        }
        Ok(Self {
            site: site.into(),
            id: id.into(),
            name: name.into(),
            api,
            version: version.into(),
        })
    }

    /// The datagram.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        format!(
            "{MAGIC}\nsite {}\nid {}\nname {}\napi {}\nversion {}\n",
            self.site, self.id, self.name, self.api, self.version
        )
        .into_bytes()
    }

    /// A datagram, held to the format exactly.
    ///
    /// # Errors
    /// It is too long, or anything about it is not exactly a beacon.
    pub fn parse(b: &[u8]) -> Result<Self, BadBeacon> {
        if b.len() > MAX_BEACON {
            return Err(BadBeacon::Oversized);
        }
        let text = std::str::from_utf8(b).map_err(|_| BadBeacon::Malformed)?;
        if !text.is_ascii() || text.contains('\r') {
            return Err(BadBeacon::Malformed);
        }
        let body = text.strip_suffix('\n').ok_or(BadBeacon::Malformed)?;
        let lines: Vec<&str> = body.split('\n').collect();
        let [magic, site, id, name, api, version] = lines.as_slice() else {
            return Err(BadBeacon::Malformed);
        };
        if *magic != MAGIC {
            return Err(BadBeacon::Malformed);
        }
        let field = |line: &str, key: &str| -> Result<String, BadBeacon> {
            line.strip_prefix(key)
                .and_then(|r| r.strip_prefix(' '))
                .map(str::to_string)
                .ok_or(BadBeacon::Malformed)
        };
        let api = port(&field(api, "api")?).ok_or(BadBeacon::Malformed)?;
        Self::new(
            &field(site, "site")?,
            &field(id, "id")?,
            &field(name, "name")?,
            api,
            &field(version, "version")?,
        )
        .map_err(|_| BadBeacon::Malformed)
    }
}

/// Whether a beacon from `ip` is looked at: private (RFC 1918), shared
/// (RFC 6598), link-local or loopback IPv4.
#[must_use]
pub fn local_source(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_private() || v4.is_link_local() || v4.is_loopback() || (a == 100 && (64..128).contains(&b))
        }
        IpAddr::V6(_) => false,
    }
}

/// A box heard on the LAN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub version: String,
    /// Its API: the beacon's source address with the port it names.
    pub addr: SocketAddr,
    /// When it was first and last heard (site time).
    pub first: Duration,
    pub last: Duration,
    /// Another address that claimed this id while it was live.
    pub conflict: Option<IpAddr>,
}

/// What became of datagrams, by outcome.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Counts {
    /// Taken in: a new peer, or a refresh.
    pub accepted: u64,
    pub oversized: u64,
    pub malformed: u64,
    /// From outside the private, link-local and loopback ranges.
    pub not_local: u64,
    /// Another site's.
    pub other_site: u64,
    /// This box's own, looped back.
    pub own: u64,
    /// A known peer again within [`MIN_GAP`].
    pub too_soon: u64,
    /// A new id while [`MAX_PEERS`] are listed.
    pub full: u64,
    /// A live id from a second address.
    pub conflict: u64,
    /// Beyond [`INTAKE`] a second, dropped unread.
    pub throttled: u64,
}

/// What one datagram did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heard {
    New,
    Refreshed,
    Oversized,
    Malformed,
    NotLocal,
    OtherSite,
    Own,
    TooSoon,
    Full,
    Conflict,
    Throttled,
}

/// The peers a box has heard, and what it dropped. No I/O: the socket
/// loop hands it datagrams with the time.
#[derive(Debug, Clone)]
pub struct Peers {
    site: String,
    own: String,
    map: BTreeMap<String, Peer>,
    counts: Counts,
    /// The current intake second and datagrams looked at in it.
    window: (u64, u32),
}

impl Peers {
    /// For a box in `site` whose own id is `own`.
    #[must_use]
    pub fn new(site: &str, own: &str) -> Self {
        Self {
            site: site.into(),
            own: own.into(),
            map: BTreeMap::new(),
            counts: Counts::default(),
            window: (0, 0),
        }
    }

    /// Takes in one datagram from `from` at site time `now`.
    pub fn datagram(&mut self, bytes: &[u8], from: SocketAddr, now: Duration) -> Heard {
        let second = now.as_secs();
        if self.window.0 == second {
            self.window.1 = self.window.1.saturating_add(1);
        } else {
            self.window = (second, 1);
        }
        let heard = if self.window.1 > INTAKE {
            Heard::Throttled
        } else if !local_source(from.ip()) {
            Heard::NotLocal
        } else {
            match Beacon::parse(bytes) {
                Err(BadBeacon::Oversized) => Heard::Oversized,
                Err(BadBeacon::Malformed) => Heard::Malformed,
                Ok(b) => self.beacon(b, from.ip(), now),
            }
        };
        let c = &mut self.counts;
        let slot = match heard {
            Heard::New | Heard::Refreshed => &mut c.accepted,
            Heard::Oversized => &mut c.oversized,
            Heard::Malformed => &mut c.malformed,
            Heard::NotLocal => &mut c.not_local,
            Heard::OtherSite => &mut c.other_site,
            Heard::Own => &mut c.own,
            Heard::TooSoon => &mut c.too_soon,
            Heard::Full => &mut c.full,
            Heard::Conflict => &mut c.conflict,
            Heard::Throttled => &mut c.throttled,
        };
        *slot = slot.saturating_add(1);
        heard
    }

    fn beacon(&mut self, b: Beacon, ip: IpAddr, now: Duration) -> Heard {
        if b.site != self.site {
            return Heard::OtherSite;
        }
        if b.id == self.own {
            return Heard::Own;
        }
        self.expire(now);
        let addr = SocketAddr::new(ip, b.api);
        if let Some(p) = self.map.get_mut(&b.id) {
            if p.addr.ip() != ip {
                p.conflict = Some(ip);
                return Heard::Conflict;
            }
            if now.saturating_sub(p.last) < MIN_GAP {
                return Heard::TooSoon;
            }
            p.addr = addr;
            p.name = b.name;
            p.version = b.version;
            p.last = now;
            return Heard::Refreshed;
        }
        if self.map.len() >= MAX_PEERS {
            return Heard::Full;
        }
        self.map.insert(
            b.id.clone(),
            Peer {
                id: b.id,
                name: b.name,
                version: b.version,
                addr,
                first: now,
                last: now,
                conflict: None,
            },
        );
        Heard::New
    }

    /// Forgets peers not heard for [`EXPIRY`] at `now`.
    pub fn expire(&mut self, now: Duration) {
        self.map.retain(|_, p| now.saturating_sub(p.last) < EXPIRY);
    }

    /// The live peers at `now`, by id.
    #[must_use]
    pub fn list(&self, now: Duration) -> Vec<Peer> {
        self.map
            .values()
            .filter(|p| now.saturating_sub(p.last) < EXPIRY)
            .cloned()
            .collect()
    }

    #[must_use]
    pub const fn counts(&self) -> Counts {
        self.counts
    }

    #[must_use]
    pub fn site(&self) -> &str {
        &self.site
    }
}

/// Where log lines go.
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// What a running site shares with the API.
#[derive(Debug)]
struct State {
    me: Beacon,
    peers: Mutex<Peers>,
    t0: Instant,
}

/// The API's view of a running site: this box's beacon and the peers.
#[derive(Debug, Clone)]
pub struct Handle(Arc<State>);

impl Handle {
    /// This box's beacon.
    #[must_use]
    pub fn me(&self) -> &Beacon {
        &self.0.me
    }

    /// The live peers, what was dropped, and the site time now.
    #[must_use]
    pub fn snapshot(&self) -> (Vec<Peer>, Counts, Duration) {
        let now = self.0.t0.elapsed();
        let p = self.0.peers.lock().unwrap_or_else(PoisonError::into_inner);
        (p.list(now), p.counts(), now)
    }
}

/// A running site: one thread sending this box's beacon and hearing the
/// others'. Stops (and joins) when dropped.
pub struct Site {
    handle: Handle,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Site {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Site")
            .field("me", self.handle.me())
            .finish_non_exhaustive()
    }
}

/// The socket a box hears beacons on: `listen`, joined to `announce`'s
/// group when that is multicast, with a TTL of 1 and loopback on (two
/// boxes on one host hear each other).
///
/// # Errors
/// The bind or the join fails.
pub fn bind(listen: SocketAddr, announce: SocketAddr) -> io::Result<UdpSocket> {
    let s = UdpSocket::bind(listen)?;
    if let IpAddr::V4(group) = announce.ip()
        && group.is_multicast()
    {
        s.join_multicast_v4(&group, &Ipv4Addr::UNSPECIFIED)?;
        s.set_multicast_ttl_v4(1)?;
        s.set_multicast_loop_v4(true)?;
    }
    Ok(s)
}

/// How long the loop waits for a datagram before checking the clock.
const TICK: Duration = Duration::from_millis(50);

impl Site {
    /// Starts sending `me` to `announce` every `every` and hearing beacons
    /// on `socket` (see [`bind`]; a test may hand in any bound socket).
    ///
    /// # Errors
    /// The socket cannot take a read timeout, or the thread cannot start.
    pub fn start(
        socket: UdpSocket,
        me: Beacon,
        announce: SocketAddr,
        every: Duration,
        log: Log,
    ) -> io::Result<Self> {
        socket.set_read_timeout(Some(TICK))?;
        let state = Arc::new(State {
            peers: Mutex::new(Peers::new(&me.site, &me.id)),
            me,
            t0: Instant::now(),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (state, stop) = (Arc::clone(&state), Arc::clone(&stop));
            thread::Builder::new()
                .name("site".into())
                .spawn(move || run(&socket, &state, announce, every, &stop, &log))?
        };
        Ok(Self {
            handle: Handle(state),
            stop,
            thread: Some(thread),
        })
    }

    #[must_use]
    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(
    socket: &UdpSocket,
    state: &State,
    announce: SocketAddr,
    every: Duration,
    stop: &AtomicBool,
    log: &Log,
) {
    let beacon = state.me.encode();
    let mut next = Duration::ZERO;
    let mut send_failed = false;
    // One byte more than a beacon may be: a datagram that fills it is too long.
    let mut buf = [0u8; MAX_BEACON + 1];
    while !stop.load(Ordering::Acquire) {
        let now = state.t0.elapsed();
        if now >= next {
            match socket.send_to(&beacon, announce) {
                Ok(_) if send_failed => {
                    send_failed = false;
                    log(&format!("site: beacons to {announce} going out again"));
                }
                Err(e) if !send_failed => {
                    send_failed = true;
                    log(&format!("site: cannot send the beacon to {announce}: {e}"));
                }
                _ => {}
            }
            next = now + every;
        }
        if let Ok((n, from)) = socket.recv_from(&mut buf) {
            let now = state.t0.elapsed();
            state
                .peers
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .datagram(&buf[..n], from, now);
        }
    }
}
