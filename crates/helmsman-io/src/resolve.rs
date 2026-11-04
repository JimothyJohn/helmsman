//! Devices named by what they are, not where they are (`device-by-identity`,
//! interview 2026-11-02): a map's `device` line may give, in place of a
//! host, one of
//!
//! - `mac=00-90-e8-7e-af-be` (six hex bytes, `-` or `:` between them): the
//!   device's Ethernet address, which a DHCP lease never changes;
//! - `cip=991:515` or `cip=991:515:0x2A629EAE` (vendor id, product code
//!   and, if given, serial number, as `ListIdentity` reports them): an
//!   EtherNet/IP device's identity;
//! - `modbus=Moxa:E1213` (`VendorName` and `ProductCode`, as Read Device
//!   Identification reports them; `%XX` for a byte that is not a letter, a
//!   digit or one of `-._~`, so `modbus=Schneider%20Electric:BMX`): a
//!   Modbus TCP device's identity, matched case-blind on the trimmed text.
//!
//! A driver resolves such an endpoint to an address when it connects, at
//! start and again after a link loss, so a device whose lease moved is
//! found again:
//!
//! - `mac=`: the host's ARP table (`/proc/net/arp` on Linux, `arp -an`
//!   elsewhere; docs/SOURCES.md). A device the host has not talked to is
//!   not in the table yet, so a miss is followed by one `ListIdentity`
//!   broadcast (every EtherNet/IP device on the LAN answers, and answering
//!   puts it in the table) and a second look. A Modbus-only device that
//!   has never been reached needs one ping, or its address, the first time.
//! - `cip=`: one `ListIdentity` broadcast ([`helmsman_proto::discover`]);
//!   the first reply whose vendor, product code (and serial, when given)
//!   match is the device; the address is the one the reply came from.
//! - `modbus=`: the identification probe ([`probe_modbus`]): Read Device
//!   Identification (FC 43/14, [`crate::modbus::identify`]) to TCP 502 of
//!   each host the ARP table lists, and nothing else: a bounded number of
//!   hosts, a few at a time, each under a deadline and the whole probe
//!   under another. A device the host has not talked to yet is not in the
//!   table, so, as for `mac=`, it needs one ping or its address the first
//!   time.
//!
//! The resolved address is what the link reports going up with, so the
//! Devices tab shows both the identity and where it was found. The
//! `Resolver` is how a test substitutes the ARP table and the sweep's
//! target.

use std::fmt::{self, Write as _};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs, UdpSocket};
use std::time::Duration;

/// The identity a `device` line may give in place of a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    /// `mac=XX-XX-XX-XX-XX-XX`.
    Mac([u8; 6]),
    /// `cip=VENDOR:PRODUCT[:SERIAL]`.
    Cip {
        vendor: u16,
        product: u16,
        serial: Option<u32>,
    },
    /// `modbus=VENDOR:PRODUCT`: Read Device Identification's `VendorName`
    /// and `ProductCode`, decoded.
    Modbus { vendor: String, product: String },
}

/// `s` with every byte but ASCII letters, digits and `-._~` as `%XX`: one
/// token of a map line, whatever a device calls itself.
#[must_use]
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// `%XX` decoded; `None` for a `%` without two hex digits or bytes that
/// are not UTF-8.
fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = b.get(i + 1..i + 3)?;
            if !hex.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            out.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Whether a device's identification text is the text a `modbus=` names:
/// trimmed, case-blind.
#[must_use]
pub fn modbus_text_eq(named: &str, reported: &str) -> bool {
    named.trim().eq_ignore_ascii_case(reported.trim())
}

impl fmt::Display for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mac(m) => write!(
                f,
                "mac={:02x}-{:02x}-{:02x}-{:02x}-{:02x}-{:02x}",
                m[0], m[1], m[2], m[3], m[4], m[5]
            ),
            Self::Cip {
                vendor,
                product,
                serial: None,
            } => write!(f, "cip={vendor}:{product}"),
            Self::Cip {
                vendor,
                product,
                serial: Some(s),
            } => write!(f, "cip={vendor}:{product}:{s:#010x}"),
            Self::Modbus { vendor, product } => {
                write!(f, "modbus={}:{}", percent_encode(vendor), percent_encode(product))
            }
        }
    }
}

/// What a `device` line's endpoint token is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// `host[:port]`, resolved by the OS as ever.
    Host(String),
    Identity(Identity),
}

/// Parses an endpoint token: an identity if it starts with `mac=` or
/// `cip=`, else a host.
///
/// # Errors
/// An identity that does not read: the shape it needs.
pub fn parse(endpoint: &str) -> Result<Endpoint, String> {
    if let Some(mac) = endpoint.strip_prefix("mac=") {
        return parse_mac(mac)
            .map(|m| Endpoint::Identity(Identity::Mac(m)))
            .ok_or_else(|| "mac= needs six hex bytes, like mac=00-90-e8-7e-af-be".to_string());
    }
    if let Some(cip) = endpoint.strip_prefix("cip=") {
        return parse_cip(cip).map(Endpoint::Identity);
    }
    if let Some(m) = endpoint.strip_prefix("modbus=") {
        return parse_modbus(m).map(Endpoint::Identity);
    }
    if endpoint.is_empty() || endpoint.chars().any(char::is_whitespace) {
        return Err("a host is one word, like 192.168.1.50:502".into());
    }
    Ok(Endpoint::Host(endpoint.to_string()))
}

/// Six hex bytes with `-` or `:` between them (one or two digits each, as
/// `arp -an` prints them).
#[must_use]
pub fn parse_mac(s: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = s.split(['-', ':']).collect();
    if parts.len() != 6 {
        return None;
    }
    let mut out = [0u8; 6];
    for (k, p) in parts.iter().enumerate() {
        if p.is_empty() || p.len() > 2 || !p.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        out[k] = u8::from_str_radix(p, 16).ok()?;
    }
    Some(out)
}

fn parse_u32(s: &str) -> Option<u32> {
    s.strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .map_or_else(|| s.parse().ok(), |h| u32::from_str_radix(h, 16).ok())
}

fn parse_cip(s: &str) -> Result<Identity, String> {
    let shape = "cip= is VENDOR:PRODUCT or VENDOR:PRODUCT:SERIAL (numbers; the serial may be hex, 0x...)";
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 && parts.len() != 3 {
        return Err(shape.into());
    }
    let small = |p: &str| u16::try_from(parse_u32(p).ok_or(shape)?).map_err(|_| shape.to_string());
    let vendor = small(parts[0])?;
    let product = small(parts[1])?;
    let serial = match parts.get(2) {
        Some(p) => Some(parse_u32(p).ok_or(shape)?),
        None => None,
    };
    Ok(Identity::Cip {
        vendor,
        product,
        serial,
    })
}

fn parse_modbus(s: &str) -> Result<Identity, String> {
    let shape = "modbus= is VENDOR:PRODUCT, as the device's identification names them (%20 for a space), like modbus=Moxa:E1213";
    let Some((v, p)) = s.split_once(':') else {
        return Err(shape.into());
    };
    if p.contains(':') || s.chars().any(char::is_whitespace) {
        return Err(shape.into());
    }
    let text = |raw: &str| {
        percent_decode(raw)
            .filter(|t| {
                !t.trim().is_empty()
                    && t.chars().count() <= crate::modbus::MAX_ID_TEXT
                    && !t.chars().any(char::is_control)
            })
            .ok_or_else(|| shape.to_string())
    };
    Ok(Identity::Modbus {
        vendor: text(v)?,
        product: text(p)?,
    })
}

/// How the identification probe ([`probe_modbus`]) is bounded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModbusProbe {
    /// The TCP port asked (502, TCP §3.1.3 p.5; a test's simulator elsewhere).
    pub port: u16,
    /// The unit id asked.
    pub unit: u8,
    /// One host's deadline: connect, requests and replies.
    pub per_host: Duration,
    /// The whole probe's: a host not started by then is not asked.
    pub total: Duration,
    /// Most hosts asked; the ARP table's first ones in address order.
    pub max_hosts: usize,
    /// Most hosts asked at once.
    pub concurrency: usize,
}

impl Default for ModbusProbe {
    fn default() -> Self {
        Self {
            port: crate::modbus::DEFAULT_PORT,
            unit: 1,
            per_host: Duration::from_millis(500),
            total: Duration::from_secs(3),
            max_hosts: 64,
            concurrency: 8,
        }
    }
}

/// How identities are looked up: the ARP table's text and where a
/// `ListIdentity` goes. [`Resolver::default`] is the host's; a test gives
/// its own.
pub struct Resolver {
    /// The host's ARP table as text, in `/proc/net/arp`'s or `arp -an`'s
    /// format.
    pub arp: Box<dyn Fn() -> io::Result<String> + Send + Sync>,
    /// Where one `ListIdentity` is sent (the broadcast address and the
    /// encapsulation port); `None`: no sweep.
    pub sweep: Option<SocketAddrV4>,
    /// How long replies are gathered.
    pub window: Duration,
    /// The Modbus identification probe for `modbus=`; `None`: no probe.
    pub modbus: Option<ModbusProbe>,
}

impl fmt::Debug for Resolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Resolver")
            .field("sweep", &self.sweep)
            .field("window", &self.window)
            .field("modbus", &self.modbus)
            .finish_non_exhaustive()
    }
}

impl Default for Resolver {
    fn default() -> Self {
        Self {
            arp: Box::new(host_arp_table),
            sweep: Some(SocketAddrV4::new(Ipv4Addr::BROADCAST, 44818)),
            window: Duration::from_millis(700),
            modbus: Some(ModbusProbe::default()),
        }
    }
}

/// The host's ARP table: `/proc/net/arp` where there is one, else what
/// `arp -an` prints.
///
/// # Errors
/// Neither could be read.
pub fn host_arp_table() -> io::Result<String> {
    if let Ok(t) = std::fs::read_to_string("/proc/net/arp") {
        return Ok(t);
    }
    let out = std::process::Command::new("arp").arg("-an").output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!("arp -an: {}", out.status)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The IPv4 address `table` (either format) lists for `mac`; incomplete
/// entries have no address and are passed over.
#[must_use]
pub fn arp_lookup(table: &str, mac: [u8; 6]) -> Option<Ipv4Addr> {
    for line in table.lines() {
        let mut ip = None;
        let mut hit = false;
        for tok in line.split_whitespace() {
            let t = tok.trim_matches(|c| c == '(' || c == ')');
            if let Ok(a) = t.parse::<Ipv4Addr>() {
                ip.get_or_insert(a);
            } else if parse_mac(t) == Some(mac) && mac != [0; 6] {
                // An all-zero MAC is Linux's incomplete entry, never a device.
                hit = true;
            }
        }
        if hit && let Some(a) = ip {
            return Some(a);
        }
    }
    None
}

/// The MAC `table` (either format) lists for `ip`, if any; an all-zero
/// MAC (Linux's incomplete entry) is no device.
#[must_use]
pub fn arp_mac_of(table: &str, ip: Ipv4Addr) -> Option<[u8; 6]> {
    for line in table.lines() {
        let mut here = false;
        let mut mac = None;
        for tok in line.split_whitespace() {
            let t = tok.trim_matches(|c| c == '(' || c == ')');
            if t.parse::<Ipv4Addr>() == Ok(ip) {
                here = true;
            } else if let Some(m) = parse_mac(t)
                && m != [0; 6]
            {
                mac.get_or_insert(m);
            }
        }
        if here && let Some(m) = mac {
            return Some(m);
        }
    }
    None
}

/// A device that answered a `ListIdentity` sweep (`POST /api/devices/discover`,
/// the Devices tab's "On the network"): what the wire gives, plus its MAC
/// when the host's ARP table has it (answering put it there).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub address: Ipv4Addr,
    pub mac: Option<[u8; 6]>,
    pub vendor_id: u16,
    pub product_code: u16,
    pub device_type: u16,
    pub revision: (u8, u8),
    pub serial: u32,
    /// The product name as text (lossy; a device's bytes need not be UTF-8).
    pub product_name: String,
}

impl Seen {
    /// The `cip=V:P:S` identity a map may name this device by.
    #[must_use]
    pub fn cip(&self) -> String {
        format!(
            "cip={}:{}:{:#010x}",
            self.vendor_id, self.product_code, self.serial
        )
    }

    /// The `mac=` identity, when the MAC is known.
    #[must_use]
    pub fn mac_text(&self) -> Option<String> {
        self.mac.map(|m| Identity::Mac(m).to_string())
    }
}

/// One `ListIdentity` sweep with `r`'s target and window: every
/// EtherNet/IP device that answered, in address order, each at most once
/// (the first reply wins), with its MAC from the ARP table read after the
/// sweep. Read-only on the network: nothing is sent but the one request.
///
/// # Errors
/// No sweep target, or a socket error.
pub fn discover(r: &Resolver) -> io::Result<Vec<Seen>> {
    let Some(target) = r.sweep else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no ListIdentity sweep target",
        ));
    };
    let found = sweep(target, r.window)?;
    let table = (r.arp)().unwrap_or_default();
    let mut out: Vec<Seen> = Vec::new();
    for f in found.found {
        let SocketAddr::V4(from) = f.from else { continue };
        if out.iter().any(|s| s.address == *from.ip()) {
            continue;
        }
        let id = f.identity;
        let name: String = String::from_utf8_lossy(&id.product_name)
            .chars()
            .filter(|c| !c.is_control())
            .take(64)
            .collect();
        out.push(Seen {
            address: *from.ip(),
            mac: arp_mac_of(&table, *from.ip()),
            vendor_id: id.vendor_id,
            product_code: id.product_code,
            device_type: id.device_type,
            revision: (id.revision_major, id.revision_minor),
            serial: id.serial_number,
            product_name: name,
        });
    }
    out.sort_by_key(|s| s.address);
    Ok(out)
}

/// One `ListIdentity` to `target`, the replies gathered for `window`.
fn sweep(target: SocketAddrV4, window: Duration) -> io::Result<helmsman_proto::discover::Sweep> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.set_broadcast(true)?;
    helmsman_proto::discover::sweep(&socket, target, window)
}

/// The hosts `table` (either format) lists with a complete entry, in
/// address order, each once: what the Modbus probe may ask. Incomplete
/// entries, group MACs (broadcast, multicast) and multicast, broadcast or
/// unspecified addresses are no device and are left out.
#[must_use]
pub fn arp_hosts(table: &str) -> Vec<(Ipv4Addr, [u8; 6])> {
    let mut out: Vec<(Ipv4Addr, [u8; 6])> = Vec::new();
    for line in table.lines() {
        let mut ip = None;
        let mut mac = None;
        for tok in line.split_whitespace() {
            let t = tok.trim_matches(|c| c == '(' || c == ')');
            if let Ok(a) = t.parse::<Ipv4Addr>() {
                ip.get_or_insert(a);
            } else if let Some(m) = parse_mac(t) {
                mac.get_or_insert(m);
            }
        }
        if let (Some(a), Some(m)) = (ip, mac)
            && m != [0; 6]
            && m[0] & 1 == 0
            && !a.is_multicast()
            && !a.is_broadcast()
            && !a.is_unspecified()
            && !out.iter().any(|(x, _)| *x == a)
        {
            out.push((a, m));
        }
    }
    out.sort_by_key(|(a, _)| *a);
    out
}

/// The hosts the probe asks for `table`: [`arp_hosts`], the first
/// [`ModbusProbe::max_hosts`] of them.
#[must_use]
pub fn probe_targets(table: &str, p: &ModbusProbe) -> Vec<(Ipv4Addr, [u8; 6])> {
    arp_hosts(table).into_iter().take(p.max_hosts).collect()
}

/// A host that answered the Modbus identification probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModbusSeen {
    pub address: Ipv4Addr,
    /// From the ARP table the host was taken from.
    pub mac: Option<[u8; 6]>,
    pub identification: crate::modbus::DeviceIdentification,
}

impl ModbusSeen {
    /// The `modbus=VENDOR:PRODUCT` identity a map may name this device by.
    #[must_use]
    pub fn identity(&self) -> Identity {
        Identity::Modbus {
            vendor: self.identification.vendor_name.clone(),
            product: self.identification.product_code.clone(),
        }
    }

    /// The `mac=` identity, when the MAC is known.
    #[must_use]
    pub fn mac_text(&self) -> Option<String> {
        self.mac.map(|m| Identity::Mac(m).to_string())
    }
}

/// The Modbus identification probe: Read Device Identification
/// ([`crate::modbus::identify`], read-only) to `r`'s port on each host of
/// `r`'s ARP table ([`arp_hosts`]) and no other, at most
/// [`ModbusProbe::max_hosts`] of them, [`ModbusProbe::concurrency`] at a
/// time, each within [`ModbusProbe::per_host`]; no host is started after
/// [`ModbusProbe::total`], so the probe ends by `total + per_host`. Hosts
/// that refuse, time out or answer something that does not read are
/// passed over. The ones that answered, in address order.
///
/// # Errors
/// No probe configured, or the ARP table could not be read.
pub fn probe_modbus(r: &Resolver) -> io::Result<Vec<ModbusSeen>> {
    probe_until(r, None)
}

/// [`probe_modbus`], starting no further host once one answers as
/// `wanted` says (a `modbus=` being resolved needs only its device).
fn probe_until(
    r: &Resolver,
    wanted: Option<&(dyn Fn(&crate::modbus::DeviceIdentification) -> bool + Sync)>,
) -> io::Result<Vec<ModbusSeen>> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Instant;

    let Some(p) = &r.modbus else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no Modbus identification probe",
        ));
    };
    let table = (r.arp)()?;
    let hosts = probe_targets(&table, p);
    let deadline = Instant::now() + p.total;
    let next = AtomicUsize::new(0);
    let done = AtomicBool::new(false);
    let found = Mutex::new(Vec::new());
    let workers = p.concurrency.clamp(1, 32).min(hosts.len());
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    if done.load(Ordering::Relaxed) {
                        break;
                    }
                    let Some(&(ip, mac)) = hosts.get(next.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    let addr = SocketAddr::V4(SocketAddrV4::new(ip, p.port));
                    if let Ok(id) = crate::modbus::identify(addr, p.unit, p.per_host.min(left)) {
                        if wanted.is_some_and(|w| w(&id)) {
                            done.store(true, Ordering::Relaxed);
                        }
                        found
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push(ModbusSeen {
                                address: ip,
                                mac: Some(mac),
                                identification: id,
                            });
                    }
                }
            });
        }
    });
    let mut out = found
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    out.sort_by_key(|s| s.address);
    Ok(out)
}

/// Where `id` is now, at `port`.
///
/// # Errors
/// Not found, with what was tried; a socket or table error.
pub fn resolve(id: &Identity, port: u16, r: &Resolver) -> io::Result<SocketAddrV4> {
    match id {
        Identity::Mac(mac) => {
            let table = (r.arp)()?;
            if let Some(ip) = arp_lookup(&table, *mac) {
                return Ok(SocketAddrV4::new(ip, port));
            }
            if let Some(target) = r.sweep {
                // Every EtherNet/IP device answers a ListIdentity, and
                // answering puts it in the ARP table.
                let _ = sweep(target, r.window);
                let table = (r.arp)()?;
                if let Some(ip) = arp_lookup(&table, *mac) {
                    return Ok(SocketAddrV4::new(ip, port));
                }
            }
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{id}: not in this host's ARP table (ping the device once, or give its address)"),
            ))
        }
        Identity::Cip {
            vendor,
            product,
            serial,
        } => {
            let Some(target) = r.sweep else {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("{id}: no ListIdentity sweep to find it with"),
                ));
            };
            let found = sweep(target, r.window)?;
            let hit = found.found.iter().find(|f| {
                f.identity.vendor_id == *vendor
                    && f.identity.product_code == *product
                    && serial.is_none_or(|s| s == f.identity.serial_number)
            });
            match (hit, found.found.len()) {
                (Some(f), _) => match f.from {
                    SocketAddr::V4(a) => Ok(SocketAddrV4::new(*a.ip(), port)),
                    SocketAddr::V6(_) => Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("{id}: answered from an IPv6 address"),
                    )),
                },
                (None, n) => Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("{id}: no such device among the {n} that answered ListIdentity"),
                )),
            }
        }
        Identity::Modbus { vendor, product } => {
            if r.modbus.is_none() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("{id}: no Modbus identification probe to find it with"),
                ));
            }
            let is_it = |d: &crate::modbus::DeviceIdentification| {
                modbus_text_eq(vendor, &d.vendor_name) && modbus_text_eq(product, &d.product_code)
            };
            let found = probe_until(r, Some(&is_it))?;
            let n = found.len();
            found
                .iter()
                .find(|s| is_it(&s.identification))
                .map(|s| SocketAddrV4::new(s.address, port))
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        format!(
                            "{id}: no such device among the {n} that answered Read Device Identification (only hosts in this host's ARP table are asked: ping the device once, or give its address)"
                        ),
                    )
                })
        }
    }
}

/// The address a driver connects to for `endpoint`: a host as the OS
/// resolves it (first IPv4; `default_port` when it names none), an
/// identity as [`resolve`] finds it.
///
/// # Errors
/// Neither way yields an address, with why.
pub fn socket_addr(endpoint: &str, default_port: u16, r: &Resolver) -> io::Result<SocketAddrV4> {
    match parse(endpoint).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))? {
        Endpoint::Identity(id) => resolve(&id, default_port, r),
        Endpoint::Host(host) => {
            let with_port = if host
                .rsplit_once(':')
                .is_some_and(|(_, p)| p.parse::<u16>().is_ok())
            {
                host
            } else {
                format!("{host}:{default_port}")
            };
            with_port
                .to_socket_addrs()?
                .find_map(|a| match a {
                    SocketAddr::V4(a) => Some(a),
                    SocketAddr::V6(_) => None,
                })
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, format!("{endpoint}: no IPv4 address"))
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINUX: &str = "IP address       HW type     Flags       HW address            Mask     Device\n\
10.0.0.1         0x1         0x2         38:94:ed:a8:22:3a     *        eth0\n\
10.0.0.57        0x1         0x2         00:90:e8:7e:af:be     *        eth0\n\
10.0.0.99        0x1         0x0         00:00:00:00:00:00     *        eth0\n";
    const MACOS: &str = "? (10.0.0.1) at 38:94:ed:a8:22:3a on en1 ifscope [ethernet]\n\
? (10.0.0.57) at 0:90:e8:7e:af:be on en1 ifscope [ethernet]\n\
? (10.0.0.60) at (incomplete) on en1 ifscope [ethernet]\n\
? (224.0.0.251) at 1:0:5e:0:0:fb on en1 ifscope permanent [ethernet]\n";
    const E1213: [u8; 6] = [0x00, 0x90, 0xe8, 0x7e, 0xaf, 0xbe];

    #[test]
    fn identities_parse_and_hosts_pass_through() {
        assert_eq!(
            parse("mac=00-90-e8-7e-af-be"),
            Ok(Endpoint::Identity(Identity::Mac(E1213)))
        );
        assert_eq!(
            parse("mac=0:90:E8:7e:AF:be"),
            Ok(Endpoint::Identity(Identity::Mac(E1213)))
        );
        assert_eq!(
            parse("cip=991:515"),
            Ok(Endpoint::Identity(Identity::Cip {
                vendor: 991,
                product: 515,
                serial: None
            }))
        );
        assert_eq!(
            parse("cip=991:515:0x2A629EAE"),
            Ok(Endpoint::Identity(Identity::Cip {
                vendor: 991,
                product: 515,
                serial: Some(0x2A62_9EAE)
            }))
        );
        assert_eq!(parse("cip=1:2:16").unwrap(), parse("cip=1:2:0x10").unwrap());
        assert_eq!(parse("10.0.0.57:502"), Ok(Endpoint::Host("10.0.0.57:502".into())));
        assert_eq!(parse("rio.local"), Ok(Endpoint::Host("rio.local".into())));
        for bad in [
            "mac=",
            "mac=00-90-e8-7e-af",
            "mac=00-90-e8-7e-af-be-01",
            "mac=00-90-e8-7e-af-zz",
            "mac=000-90-e8-7e-af-be",
            "mac=00 90 e8 7e af be",
            "cip=",
            "cip=991",
            "cip=991:515:1:2",
            "cip=70000:515",
            "cip=991:-1",
            "cip=moxa:e1213",
            "cip=991:515:0x",
            "",
            "a b",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        assert_eq!(Identity::Mac(E1213).to_string(), "mac=00-90-e8-7e-af-be");
        assert_eq!(
            Identity::Cip {
                vendor: 991,
                product: 515,
                serial: Some(0x2A62_9EAE)
            }
            .to_string(),
            "cip=991:515:0x2a629eae"
        );
    }

    #[test]
    fn the_arp_table_reads_in_both_formats_and_skips_incomplete_entries() {
        assert_eq!(arp_lookup(LINUX, E1213), Some(Ipv4Addr::new(10, 0, 0, 57)));
        assert_eq!(arp_lookup(MACOS, E1213), Some(Ipv4Addr::new(10, 0, 0, 57)));
        assert_eq!(
            arp_lookup(LINUX, [0; 6]),
            None,
            "an incomplete entry is no address"
        );
        assert_eq!(arp_lookup(MACOS, [1, 2, 3, 4, 5, 6]), None);
        assert_eq!(arp_lookup("", E1213), None);
        assert_eq!(
            arp_lookup(&"garbage line with 10.0.0.57 and no mac\n".repeat(1000), E1213),
            None
        );
        assert_eq!(
            arp_lookup("00:90:e8:7e:af:be 10.0.0.57\n", E1213),
            Some(Ipv4Addr::new(10, 0, 0, 57))
        );
    }

    fn fixed(table: &'static str) -> Resolver {
        Resolver {
            arp: Box::new(move || Ok(table.to_string())),
            sweep: None,
            window: Duration::from_millis(10),
            modbus: None,
        }
    }

    #[test]
    fn a_mac_resolves_through_the_table_and_says_so_when_absent() {
        let r = fixed(MACOS);
        assert_eq!(
            socket_addr("mac=00-90-e8-7e-af-be", 502, &r).unwrap(),
            "10.0.0.57:502".parse().unwrap()
        );
        let e = socket_addr("mac=00-90-e8-7e-af-01", 502, &r).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        assert!(e.to_string().contains("ARP table"), "{e}");
        let e = socket_addr("mac=nope", 502, &r).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
        let e = socket_addr("cip=991:515", 502, &r).unwrap_err();
        assert!(e.to_string().contains("no ListIdentity sweep"), "{e}");
        // A host still goes through the OS, port added when missing.
        assert_eq!(
            socket_addr("127.0.0.1", 502, &r).unwrap(),
            "127.0.0.1:502".parse().unwrap()
        );
        assert_eq!(
            socket_addr("127.0.0.1:1502", 502, &r).unwrap(),
            "127.0.0.1:1502".parse().unwrap()
        );
        let failing = Resolver {
            arp: Box::new(|| Err(io::Error::other("no arp"))),
            sweep: None,
            window: Duration::from_millis(10),
            modbus: None,
        };
        assert!(socket_addr("mac=00-90-e8-7e-af-be", 502, &failing).is_err());
    }
}

#[cfg(test)]
mod discover_tests {
    use super::*;

    const LINUX: &str = "IP address       HW type     Flags       HW address            Mask     Device\n\
10.0.0.57        0x1         0x2         00:90:e8:7e:af:be     *        eth0\n\
10.0.0.99        0x1         0x0         00:00:00:00:00:00     *        eth0\n";
    const MACOS: &str = "? (10.0.0.57) at 0:90:e8:7e:af:be on en1 ifscope [ethernet]\n\
? (10.0.0.60) at (incomplete) on en1 ifscope [ethernet]\n";
    const E1213: [u8; 6] = [0x00, 0x90, 0xe8, 0x7e, 0xaf, 0xbe];

    #[test]
    fn the_mac_of_an_address_reads_from_either_table() {
        assert_eq!(arp_mac_of(LINUX, Ipv4Addr::new(10, 0, 0, 57)), Some(E1213));
        assert_eq!(arp_mac_of(MACOS, Ipv4Addr::new(10, 0, 0, 57)), Some(E1213));
        assert_eq!(arp_mac_of(LINUX, Ipv4Addr::new(10, 0, 0, 99)), None, "incomplete");
        assert_eq!(arp_mac_of(MACOS, Ipv4Addr::new(10, 0, 0, 60)), None, "incomplete");
        assert_eq!(arp_mac_of(MACOS, Ipv4Addr::new(10, 0, 0, 1)), None);
        assert_eq!(arp_mac_of("", Ipv4Addr::new(10, 0, 0, 57)), None);
        assert_eq!(
            arp_mac_of("garbage \u{0} ( ) 10.0.0.57", Ipv4Addr::new(10, 0, 0, 57)),
            None
        );
    }

    #[test]
    fn a_seen_device_names_itself_by_identity() {
        let s = Seen {
            address: Ipv4Addr::new(10, 0, 0, 57),
            mac: Some(E1213),
            vendor_id: 991,
            product_code: 515,
            device_type: 12,
            revision: (3, 1),
            serial: 0x2A62_9EAE,
            product_name: "ioLogik E1213".into(),
        };
        assert_eq!(s.cip(), "cip=991:515:0x2a629eae");
        assert_eq!(s.mac_text().as_deref(), Some("mac=00-90-e8-7e-af-be"));
        assert!(
            discover(&Resolver {
                arp: Box::new(|| Ok(String::new())),
                sweep: None,
                window: Duration::from_millis(10),
                modbus: None,
            })
            .is_err()
        );
    }
}
