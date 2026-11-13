//! The server: one address per Ethernet address, from a pool, kept.
//!
//! [`Server::handle`] takes a decoded request and `now` (seconds since the
//! Unix epoch, so a lease file means the same after a restart) and returns
//! the reply, if one is due, with where it goes ([`Dest`], RFC 2131 4.1).
//!
//! **Which address a client gets.** In this order: its reservation; the
//! address it held before (in any state, so a block keeps its address
//! across power cycles and box restarts); the address it asks for, when
//! that is free in the pool; the lowest free address in the pool; and
//! only then one taken back: an offer nobody took, then a released
//! address, then an expired lease, oldest first. A lease that has not
//! expired is never taken from its holder, so a flood of DISCOVERs from
//! made-up addresses can fill the pool but not move a device off its
//! address (tests/server.rs).
//!
//! **What it answers.** DISCOVER (OFFER), REQUEST in each of its four
//! forms (RFC 2131 4.3.2: ACK or NAK, or silence when the request is for
//! another server or for a client it has no record of), DECLINE and
//! RELEASE (no reply), and a BOOTP request (a BOOTREPLY with a binding
//! that does not expire: a BOOTP client never renews). INFORM, relayed
//! requests (`giaddr` set), replies, and anything not Ethernet are
//! dropped.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::net::Ipv4Addr;

use crate::message::{BOOTREPLY, BOOTREQUEST, FLAG_BROADCAST, Kind, Mac, Message, opt};

/// How long an offer holds its address for the client it was made to.
pub const OFFER_HOLD_S: u64 = 60;
/// How long an address a client declined (someone else answers ARP for
/// it) is kept out of the pool.
pub const DECLINE_HOLD_S: u64 = 600;
/// `until` of a binding that does not expire (BOOTP).
pub const FOREVER: u64 = u64::MAX;

/// What the server hands out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The box's own address on the device network: the server
    /// identifier (option 54) and `siaddr`.
    pub server: Ipv4Addr,
    /// The network's mask (option 1).
    pub netmask: Ipv4Addr,
    /// A default gateway to hand out (option 3); `None` hands out none,
    /// which keeps a device network a closed segment.
    pub router: Option<Ipv4Addr>,
    /// First and last address of the pool, both included.
    pub pool: (Ipv4Addr, Ipv4Addr),
    /// Lease length in seconds (option 51).
    pub lease_s: u32,
    /// Fixed addresses by Ethernet address. A reserved address may lie
    /// outside the pool but must be on the network.
    pub reserved: BTreeMap<Mac, Ipv4Addr>,
    /// Whether to answer BOOTP clients (no option 53).
    pub bootp: bool,
}

/// Why a [`Config`] cannot run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigError {
    /// The netmask's ones are not contiguous.
    Netmask,
    /// The pool's first address is after its last.
    PoolOrder,
    /// A pool end, or a reservation, is not on the server's network, or is
    /// its network or broadcast address.
    OffNetwork(Ipv4Addr),
    /// The pool holds the server's own address.
    PoolHoldsServer,
    /// Two Ethernet addresses reserve one address.
    DoubleReservation(Ipv4Addr),
    /// A lease of zero seconds.
    LeaseZero,
}

impl core::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Netmask => write!(f, "the netmask's ones are not contiguous"),
            Self::PoolOrder => write!(f, "the pool's first address is after its last"),
            Self::OffNetwork(ip) => write!(f, "{ip} is not a host address on the server's network"),
            Self::PoolHoldsServer => write!(f, "the pool holds the server's own address"),
            Self::DoubleReservation(ip) => write!(f, "{ip} is reserved twice"),
            Self::LeaseZero => write!(f, "a lease of zero seconds"),
        }
    }
}

impl core::error::Error for ConfigError {}

impl Config {
    /// Check the configuration.
    ///
    /// # Errors
    ///
    /// The first [`ConfigError`] found.
    pub fn check(&self) -> Result<(), ConfigError> {
        let mask = u32::from(self.netmask);
        if mask.leading_ones() + mask.trailing_zeros() != 32 {
            return Err(ConfigError::Netmask);
        }
        if self.lease_s == 0 {
            return Err(ConfigError::LeaseZero);
        }
        let (first, last) = self.pool;
        if u32::from(first) > u32::from(last) {
            return Err(ConfigError::PoolOrder);
        }
        for ip in [first, last].into_iter().chain(self.reserved.values().copied()) {
            if !self.is_host(ip) {
                return Err(ConfigError::OffNetwork(ip));
            }
        }
        if self.in_pool(self.server) {
            return Err(ConfigError::PoolHoldsServer);
        }
        let mut seen = Vec::new();
        for ip in self.reserved.values() {
            if seen.contains(ip) || *ip == self.server {
                return Err(ConfigError::DoubleReservation(*ip));
            }
            seen.push(*ip);
        }
        Ok(())
    }

    fn in_pool(&self, ip: Ipv4Addr) -> bool {
        (u32::from(self.pool.0)..=u32::from(self.pool.1)).contains(&u32::from(ip))
    }

    /// On the server's network, and neither its network nor its broadcast
    /// address.
    fn is_host(&self, ip: Ipv4Addr) -> bool {
        let mask = u32::from(self.netmask);
        let net = u32::from(self.server) & mask;
        let ip = u32::from(ip);
        ip & mask == net && ip != net && ip != net | !mask
    }

    fn reserved_for_other(&self, ip: Ipv4Addr, mac: Mac) -> bool {
        self.reserved.iter().any(|(m, r)| *r == ip && *m != mac)
    }
}

/// Where a binding stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Offered, not yet requested; held until `until`.
    Offered,
    /// Leased until `until` ([`FOREVER`] for BOOTP).
    Bound,
    /// Given back by the client; the address stays its first choice.
    Released,
}

/// One client's address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub ip: Ipv4Addr,
    pub state: State,
    /// Seconds since the Unix epoch.
    pub until: u64,
}

impl Binding {
    /// Whether this binding still holds its address against other clients.
    #[must_use]
    pub fn holds(&self, now: u64) -> bool {
        self.state != State::Released && self.until > now
    }
}

/// Where a reply goes (RFC 2131 4.1, with relayed requests dropped).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dest {
    /// 255.255.255.255, UDP 68.
    Broadcast,
    /// Unicast to `ip`, UDP 68. With `mac` the client has no address yet
    /// and the sender must address the frame to `mac` without asking ARP;
    /// a sender that cannot do that broadcasts instead, which every client
    /// accepts.
    Unicast { ip: Ipv4Addr, mac: Option<Mac> },
}

/// A reply and where to send it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub dest: Dest,
    pub message: Message,
}

/// What happened, for the box's log and the Devices tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Offered(Mac, Ipv4Addr),
    Leased(Mac, Ipv4Addr),
    Refused(Mac),
    Declined(Mac, Ipv4Addr),
    Released(Mac, Ipv4Addr),
    /// A DISCOVER found no address to offer.
    PoolFull(Mac),
}

/// The server.
#[derive(Debug, Clone)]
pub struct Server {
    config: Config,
    bindings: BTreeMap<Mac, Binding>,
    /// Declined addresses, out of the pool until the time given.
    declined: BTreeMap<Ipv4Addr, u64>,
    events: Vec<Event>,
}

impl Server {
    /// A server with no bindings.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] from [`Config::check`].
    pub fn new(config: Config) -> Result<Self, ConfigError> {
        config.check()?;
        Ok(Self {
            config,
            bindings: BTreeMap::new(),
            declined: BTreeMap::new(),
            events: Vec::new(),
        })
    }

    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Every binding, by Ethernet address.
    #[must_use]
    pub fn bindings(&self) -> &BTreeMap<Mac, Binding> {
        &self.bindings
    }

    /// Put back bindings read from the lease file ([`crate::leases`]).
    /// One whose address is off the network, reserved for another client,
    /// or already held by an earlier one is skipped; the count of those is
    /// returned.
    pub fn restore(&mut self, bindings: impl IntoIterator<Item = (Mac, Binding)>) -> usize {
        let mut skipped = 0;
        for (mac, b) in bindings {
            let taken = self.bindings.iter().any(|(m, o)| *m != mac && o.ip == b.ip);
            if !self.config.is_host(b.ip)
                || b.ip == self.config.server
                || self.config.reserved_for_other(b.ip, mac)
                || taken
                || b.state == State::Offered
            {
                skipped += 1;
                continue;
            }
            self.bindings.insert(mac, b);
        }
        skipped
    }

    /// Forget a client: its address goes back to the pool.
    pub fn forget(&mut self, mac: Mac) -> Option<Binding> {
        self.bindings.remove(&mac)
    }

    /// The events since the last call.
    pub fn take_events(&mut self) -> Vec<Event> {
        core::mem::take(&mut self.events)
    }

    /// Answer one request.
    pub fn handle(&mut self, now: u64, req: &Message) -> Option<Reply> {
        if req.op != BOOTREQUEST || !req.giaddr.is_unspecified() {
            return None;
        }
        let mac = req.mac()?;
        if !req.is_dhcp() {
            return if self.config.bootp {
                self.bootp(now, req, mac)
            } else {
                None
            };
        }
        match req.kind()? {
            Kind::Discover => self.discover(now, req, mac),
            Kind::Request => self.request(now, req, mac),
            Kind::Decline => {
                self.decline(now, req, mac);
                None
            }
            Kind::Release => {
                self.release(req, mac);
                None
            }
            Kind::Inform | Kind::Offer | Kind::Ack | Kind::Nak => None,
        }
    }

    fn discover(&mut self, now: u64, req: &Message, mac: Mac) -> Option<Reply> {
        let Some(ip) = self.choose(now, mac, req.option_ip(opt::REQUESTED_IP)) else {
            self.events.push(Event::PoolFull(mac));
            return None;
        };
        let keep = self
            .bindings
            .get(&mac)
            .filter(|b| b.ip == ip && b.holds(now) && b.state == State::Bound);
        if keep.is_none() {
            let until = now.saturating_add(OFFER_HOLD_S);
            self.bindings.insert(
                mac,
                Binding {
                    ip,
                    state: State::Offered,
                    until,
                },
            );
        }
        self.events.push(Event::Offered(mac, ip));
        Some(self.reply(req, Kind::Offer, ip, true))
    }

    fn request(&mut self, now: u64, req: &Message, mac: Mac) -> Option<Reply> {
        let server_id = req.option_ip(opt::SERVER_ID);
        let requested = req.option_ip(opt::REQUESTED_IP);
        let ours = self.bindings.get(&mac).copied();
        let (want, has_record) = match (server_id, requested, req.ciaddr.is_unspecified()) {
            // SELECTING: the client picked a server.
            (Some(id), _, _) => {
                if id != self.config.server {
                    // Another server's offer won: ours is free again.
                    if ours.is_some_and(|b| b.state == State::Offered) {
                        self.bindings.remove(&mac);
                    }
                    return None;
                }
                (requested.unwrap_or(req.ciaddr), ours.is_some())
            }
            // INIT-REBOOT: the client asks to keep the address it had.
            (None, Some(ip), true) => {
                if !self.config.is_host(ip) {
                    return Some(self.nak(mac, req));
                }
                (ip, ours.is_some())
            }
            // RENEWING or REBINDING: ciaddr is the address in use.
            (None, _, false) => (req.ciaddr, ours.is_some()),
            (None, None, true) => return None,
        };
        if !has_record {
            // No record of this client. A renewal of a free address in
            // the pool is taken back on (the box lost its file); anything
            // else is for some other server to answer.
            let renewing = server_id.is_none() && !req.ciaddr.is_unspecified();
            if !(renewing && self.free_for(now, mac, want) && self.config.in_pool(want)) {
                return if server_id.is_some() {
                    Some(self.nak(mac, req))
                } else {
                    None
                };
            }
        } else if ours.is_some_and(|b| b.ip != want) || !self.free_for(now, mac, want) {
            return Some(self.nak(mac, req));
        }
        let until = now.saturating_add(u64::from(self.config.lease_s));
        // Another client's lapsed binding of this address goes: one
        // address, one record.
        self.bindings.retain(|m, b| *m == mac || b.ip != want);
        self.bindings.insert(
            mac,
            Binding {
                ip: want,
                state: State::Bound,
                until,
            },
        );
        self.events.push(Event::Leased(mac, want));
        let mut reply = self.reply(req, Kind::Ack, want, true);
        if !req.ciaddr.is_unspecified() {
            reply.dest = Dest::Unicast {
                ip: req.ciaddr,
                mac: None,
            };
        }
        Some(reply)
    }

    fn decline(&mut self, now: u64, req: &Message, mac: Mac) {
        if req.option_ip(opt::SERVER_ID) != Some(self.config.server) {
            return;
        }
        let Some(ip) = req.option_ip(opt::REQUESTED_IP) else {
            return;
        };
        if self.bindings.get(&mac).is_some_and(|b| b.ip == ip) {
            self.bindings.remove(&mac);
            self.declined.insert(ip, now.saturating_add(DECLINE_HOLD_S));
            self.events.push(Event::Declined(mac, ip));
        }
    }

    fn release(&mut self, req: &Message, mac: Mac) {
        if req.option_ip(opt::SERVER_ID) != Some(self.config.server) {
            return;
        }
        if let Some(b) = self
            .bindings
            .get_mut(&mac)
            .filter(|b| b.ip == req.ciaddr && b.state == State::Bound)
        {
            b.state = State::Released;
            self.events.push(Event::Released(mac, req.ciaddr));
        }
    }

    fn bootp(&mut self, now: u64, req: &Message, mac: Mac) -> Option<Reply> {
        let ip = match self.bindings.get(&mac) {
            Some(b) if b.state == State::Bound && b.until == FOREVER => b.ip,
            _ => {
                let Some(ip) = self.choose(now, mac, None) else {
                    self.events.push(Event::PoolFull(mac));
                    return None;
                };
                ip
            }
        };
        self.bindings.insert(
            mac,
            Binding {
                ip,
                state: State::Bound,
                until: FOREVER,
            },
        );
        self.events.push(Event::Leased(mac, ip));
        let mut m = Message::empty(BOOTREPLY);
        self.fill(&mut m, req, ip);
        m.cookie = req.cookie;
        if m.cookie {
            m.set_option(opt::SUBNET_MASK, &self.config.netmask.octets());
            if let Some(r) = self.config.router {
                m.set_option(opt::ROUTER, &r.octets());
            }
        }
        Some(Reply {
            dest: Self::dest(req, ip),
            message: m,
        })
    }

    /// The address to offer `mac`, by the order in the module docs.
    fn choose(&mut self, now: u64, mac: Mac, asked: Option<Ipv4Addr>) -> Option<Ipv4Addr> {
        self.declined.retain(|_, until| *until > now);
        if let Some(ip) = self.config.reserved.get(&mac).copied() {
            return self.free_for(now, mac, ip).then_some(ip);
        }
        if let Some(b) = self.bindings.get(&mac).filter(|b| self.config.in_pool(b.ip))
            && self.free_for(now, mac, b.ip)
        {
            return Some(b.ip);
        }
        if let Some(ip) = asked.filter(|ip| self.config.in_pool(*ip))
            && self.unused(ip)
            && self.free_for(now, mac, ip)
        {
            return Some(ip);
        }
        let (first, last) = (u32::from(self.config.pool.0), u32::from(self.config.pool.1));
        if let Some(ip) = (first..=last)
            .map(Ipv4Addr::from)
            .find(|ip| self.unused(*ip) && self.free_for(now, mac, *ip))
        {
            return Some(ip);
        }
        // Take one back: an offer nobody took, a released address, an
        // expired lease; the oldest first.
        let rank = |b: &Binding| match b.state {
            State::Offered => 0,
            State::Released => 1,
            State::Bound => 2,
        };
        let victim = self
            .bindings
            .iter()
            .filter(|(m, b)| **m != mac && !b.holds(now) && self.config.in_pool(b.ip))
            .filter(|(_, b)| !self.declined.contains_key(&b.ip) && !self.config.reserved_for_other(b.ip, mac))
            .min_by_key(|(_, b)| (rank(b), b.until))
            .map(|(m, b)| (*m, b.ip))?;
        self.bindings.remove(&victim.0);
        Some(victim.1)
    }

    /// No binding of any client names `ip`.
    fn unused(&self, ip: Ipv4Addr) -> bool {
        !self.bindings.values().any(|b| b.ip == ip)
    }

    /// `ip` may go to `mac` now: a host address that is not the server's,
    /// not declined, not reserved for another client, not held by another.
    fn free_for(&self, now: u64, mac: Mac, ip: Ipv4Addr) -> bool {
        self.config.is_host(ip)
            && ip != self.config.server
            && self.declined.get(&ip).is_none_or(|until| *until <= now)
            && !self.config.reserved_for_other(ip, mac)
            && !self
                .bindings
                .iter()
                .any(|(m, b)| *m != mac && b.ip == ip && b.holds(now))
    }

    fn fill(&self, m: &mut Message, req: &Message, yiaddr: Ipv4Addr) {
        m.htype = req.htype;
        m.hlen = req.hlen;
        m.xid = req.xid;
        m.flags = req.flags & FLAG_BROADCAST;
        m.yiaddr = yiaddr;
        m.siaddr = self.config.server;
        m.chaddr = req.chaddr;
    }

    fn reply(&self, req: &Message, kind: Kind, ip: Ipv4Addr, lease: bool) -> Reply {
        let mut m = Message::empty(BOOTREPLY);
        self.fill(&mut m, req, ip);
        m.set_option(opt::MESSAGE_TYPE, &[kind.code()]);
        m.set_option(opt::SERVER_ID, &self.config.server.octets());
        if lease {
            let l = self.config.lease_s;
            m.set_option(opt::LEASE_TIME, &l.to_be_bytes());
            m.set_option(opt::RENEWAL_TIME, &(l / 2).to_be_bytes());
            m.set_option(opt::REBINDING_TIME, &(l / 8 * 7).to_be_bytes());
        }
        m.set_option(opt::SUBNET_MASK, &self.config.netmask.octets());
        if let Some(r) = self.config.router {
            m.set_option(opt::ROUTER, &r.octets());
        }
        Reply {
            dest: Self::dest(req, ip),
            message: m,
        }
    }

    fn nak(&mut self, mac: Mac, req: &Message) -> Reply {
        self.events.push(Event::Refused(mac));
        let mut m = Message::empty(BOOTREPLY);
        self.fill(&mut m, req, Ipv4Addr::UNSPECIFIED);
        m.siaddr = Ipv4Addr::UNSPECIFIED;
        m.set_option(opt::MESSAGE_TYPE, &[Kind::Nak.code()]);
        m.set_option(opt::SERVER_ID, &self.config.server.octets());
        Reply {
            dest: Dest::Broadcast,
            message: m,
        }
    }

    fn dest(req: &Message, yiaddr: Ipv4Addr) -> Dest {
        if !req.ciaddr.is_unspecified() {
            Dest::Unicast {
                ip: req.ciaddr,
                mac: None,
            }
        } else if req.flags & FLAG_BROADCAST != 0 {
            Dest::Broadcast
        } else {
            Dest::Unicast {
                ip: yiaddr,
                mac: req.mac(),
            }
        }
    }
}
