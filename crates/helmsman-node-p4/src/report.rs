//! The node's report line over UDP to a log sink on the Studio
//! (scripts/node-log-sink.py): on power over Ethernet there is no USB serial to read.

use embassy_net::udp::UdpSocket;
use embassy_net::{IpAddress, IpEndpoint, Ipv4Address};

/// One line to the sink, best effort.
pub async fn send_line(udp: &mut UdpSocket<'_>, sink: Option<IpEndpoint>, line: &str) {
    if let Some(to) = sink {
        let _ = udp.send_to(line.as_bytes(), to).await;
    }
}

/// `a.b.c.d`.
#[must_use]
pub fn parse_ipv4(addr: &str) -> Option<Ipv4Address> {
    let mut octets = [0u8; 4];
    let mut n = 0;
    for part in addr.split('.') {
        if n == 4 {
            return None;
        }
        octets[n] = part.parse().ok()?;
        n += 1;
    }
    (n == 4).then(|| Ipv4Address::new(octets[0], octets[1], octets[2], octets[3]))
}

/// `a.b.c.d/nn`.
#[must_use]
pub fn parse_cidr(s: &str) -> Option<embassy_net::Ipv4Cidr> {
    let (ip, prefix) = s.split_once('/')?;
    let prefix: u8 = prefix.trim().parse().ok().filter(|p| *p <= 32)?;
    Some(embassy_net::Ipv4Cidr::new(parse_ipv4(ip)?, prefix))
}

/// The build's addressing: `NODE_STATIC_IP=a.b.c.d/nn` (and `NODE_GATEWAY`)
/// at build time for a fixed address, else DHCP. The settings record
/// replaces this.
///
/// # Errors
/// The text of a `NODE_STATIC_IP` that is not `a.b.c.d/nn`.
// The DNS list is heapless's Vec, which this crate does not name.
#[allow(clippy::default_trait_access)]
pub fn build_addressing(
    static_ip: Option<&str>,
    gateway: Option<&str>,
) -> Result<crate::ethernet::Addressing, &'static str> {
    match static_ip {
        None => Ok(crate::ethernet::Addressing::Dhcp),
        Some(cidr) => {
            let address = parse_cidr(cidr).ok_or("NODE_STATIC_IP is not a.b.c.d/nn")?;
            Ok(crate::ethernet::Addressing::Static(embassy_net::StaticConfigV4 {
                address,
                gateway: gateway.and_then(parse_ipv4),
                dns_servers: Default::default(),
            }))
        }
    }
}

/// `a.b.c.d` with `port`.
#[must_use]
pub fn parse_endpoint(addr: &str, port: u16) -> Option<IpEndpoint> {
    parse_ipv4(addr).map(|ip| IpEndpoint::new(IpAddress::Ipv4(ip), port))
}

/// `a.b.c.d:port`.
#[must_use]
pub fn parse_endpoint_port(s: &str) -> Option<IpEndpoint> {
    let (host, port) = s.rsplit_once(':')?;
    parse_endpoint(host, port.parse().ok()?)
}
