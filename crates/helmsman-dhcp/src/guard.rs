//! Is another DHCP server on this network?
//!
//! A second server on a network someone else runs hands out addresses
//! that collide with theirs; on a plant network that is an outage. So the
//! box runs [`Server`](crate::Server) only where nobody else answers:
//!
//! 1. Before it serves, it broadcasts [`probe`] (a DISCOVER from the box's
//!    own Ethernet address) and listens on UDP 68 for a while; any reply
//!    that [`foreign`] names keeps the server off.
//! 2. While it serves, every BOOTREPLY seen on the interface goes through
//!    [`foreign`]; one from another server stops it.
//!
//! The box says which server answered, on the Devices tab and in its log,
//! and does not serve until an engineer starts it again.

use core::net::Ipv4Addr;

use crate::message::{BOOTREPLY, FLAG_BROADCAST, Kind, Mac, Message, opt};

/// A DISCOVER from `mac` with transaction id `xid`, asking for a
/// broadcast reply so a box without an address on the network hears it.
#[must_use]
pub fn probe(xid: u32, mac: Mac) -> Message {
    let mut m = Message::empty(crate::message::BOOTREQUEST);
    m.xid = xid;
    m.flags = FLAG_BROADCAST;
    m.chaddr[..6].copy_from_slice(&mac.0);
    m.set_option(opt::MESSAGE_TYPE, &[Kind::Discover.code()]);
    m
}

/// Another server, as a reply shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Foreign {
    /// Its server identifier (option 54), or for BOOTP the address the
    /// datagram came from.
    pub server: Ipv4Addr,
    /// The address it offered or gave.
    pub yiaddr: Ipv4Addr,
}

/// Whether `reply`, a datagram from `src`, came from a server other than
/// `own` (the box's address on the network; `None` before it has one).
/// Requests and replies of our own say no.
#[must_use]
pub fn foreign(reply: &Message, src: Ipv4Addr, own: Option<Ipv4Addr>) -> Option<Foreign> {
    if reply.op != BOOTREPLY {
        return None;
    }
    let server = reply.option_ip(opt::SERVER_ID).unwrap_or(src);
    if Some(server) == own || (Some(src) == own && reply.option(opt::SERVER_ID).is_none()) {
        return None;
    }
    if reply.is_dhcp() && !matches!(reply.kind(), Some(Kind::Offer | Kind::Ack | Kind::Nak)) {
        return None;
    }
    Some(Foreign {
        server,
        yiaddr: reply.yiaddr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN: Ipv4Addr = Ipv4Addr::new(192, 168, 3, 1);
    const IT: Ipv4Addr = Ipv4Addr::new(192, 168, 3, 254);

    fn offer(server: Ipv4Addr) -> Message {
        let mut m = Message::empty(BOOTREPLY);
        m.yiaddr = Ipv4Addr::new(192, 168, 3, 50);
        m.set_option(opt::MESSAGE_TYPE, &[Kind::Offer.code()]);
        m.set_option(opt::SERVER_ID, &server.octets());
        m
    }

    #[test]
    fn the_probe_is_a_broadcast_discover_from_the_box() {
        let mac = Mac([2, 0, 0, 0, 0, 1]);
        let p = Message::parse(&probe(7, mac).encode()).unwrap();
        assert_eq!(
            (p.kind(), p.xid, p.mac(), p.flags),
            (Some(Kind::Discover), 7, Some(mac), FLAG_BROADCAST)
        );
    }

    #[test]
    fn another_servers_offer_is_foreign_and_ours_is_not() {
        assert_eq!(
            foreign(&offer(IT), IT, Some(OWN)),
            Some(Foreign {
                server: IT,
                yiaddr: offer(IT).yiaddr
            })
        );
        assert_eq!(foreign(&offer(IT), IT, None).map(|f| f.server), Some(IT));
        assert_eq!(foreign(&offer(OWN), OWN, Some(OWN)), None);
        // A relay forwarding another server's reply: the id says who.
        assert_eq!(foreign(&offer(IT), OWN, Some(OWN)).map(|f| f.server), Some(IT));
    }

    #[test]
    fn a_bootp_reply_names_its_sender() {
        let mut m = Message::empty(BOOTREPLY);
        m.cookie = false;
        assert_eq!(foreign(&m, IT, Some(OWN)).map(|f| f.server), Some(IT));
        assert_eq!(foreign(&m, OWN, Some(OWN)), None);
    }

    #[test]
    fn requests_are_not_servers() {
        assert_eq!(foreign(&probe(1, Mac([2, 0, 0, 0, 0, 1])), IT, Some(OWN)), None);
    }
}
