//! Devices named by what they are, not where they are (`device-by-identity`,
//! interview 2026-11-02): a map's `device` line may give, in place of a
//! host, one of
//!
//! - `mac=00-90-e8-7e-af-be` (six hex bytes, `-` or `:` between them): the
//!   device's Ethernet address, which a DHCP lease never changes;
//! - `cip=991:515` or `cip=991:515:0x2A629EAE` (vendor id, product code
//!   and, if given, serial number, as `ListIdentity` reports them): an
//!   EtherNet/IP device's identity.
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
//!
//! The resolved address is what the link reports going up with, so the
//! Devices tab shows both the identity and where it was found. The
//! `Resolver` is how a test substitutes the ARP table and the sweep's
//! target.

use std::fmt;
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
}

impl fmt::Debug for Resolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Resolver")
            .field("sweep", &self.sweep)
            .field("window", &self.window)
            .finish_non_exhaustive()
    }
}

impl Default for Resolver {
    fn default() -> Self {
        Self {
            arp: Box::new(host_arp_table),
            sweep: Some(SocketAddrV4::new(Ipv4Addr::BROADCAST, 44818)),
            window: Duration::from_millis(700),
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

/// One `ListIdentity` to `target`, the replies gathered for `window`.
fn sweep(target: SocketAddrV4, window: Duration) -> io::Result<helmsman_proto::discover::Sweep> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.set_broadcast(true)?;
    helmsman_proto::discover::sweep(&socket, target, window)
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
        };
        assert!(socket_addr("mac=00-90-e8-7e-af-be", 502, &failing).is_err());
    }
}
