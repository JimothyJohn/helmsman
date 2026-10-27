//! UDP discovery: broadcast `ListIdentity` and collect the replies.
//!
//! The command is allowed over UDP (`OpENer` `encap.c`, "allowed for both UDP
//! and TCP"); adapters delay their reply by a random interval to avoid a
//! broadcast storm (`OpENer` `EncapsulationInit`, "random delayed response
//! message generation ... as suggested in the spec"), so a listen window of
//! a couple of seconds is normal.

use std::io;
use std::net::{SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use crate::encap::{self, Command, Frame};
use crate::identity::{IdentityItem, decode_list_identity};

/// One device that answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Where the reply came from. Usually, but not always, matches
    /// `identity.socket_address`; NAT and multi-homed devices differ.
    pub from: SocketAddr,
    pub identity: IdentityItem,
}

/// Why a reply was ignored. Surfaced rather than dropped so a flaky device
/// shows up in logs instead of vanishing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected {
    /// The datagram did not decode as an encapsulation frame.
    BadFrame(crate::DecodeError),
    /// The frame was not a `ListIdentity` reply, or its context did not match ours.
    NotOurs,
    /// The frame decoded but its payload did not.
    BadPayload(crate::DecodeError),
}

/// Result of one discovery sweep.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Sweep {
    pub found: Vec<Found>,
    pub rejected: Vec<(SocketAddr, Rejected)>,
}

/// Send one `ListIdentity` to `target` from `socket` and gather replies until
/// `window` elapses. `target` is usually the subnet broadcast or
/// `255.255.255.255:44818`; set `SO_BROADCAST` on the socket first for those.
///
/// # Errors
/// Any socket error other than a read timeout.
pub fn sweep(socket: &UdpSocket, target: SocketAddrV4, window: Duration) -> io::Result<Sweep> {
    let context = sender_context();
    socket.send_to(&encap::list_identity_request(context), target)?;
    collect(socket, context, window)
}

/// Gather replies to a request that carried `context`, until `window` elapses.
///
/// # Errors
/// Any socket error other than a read timeout.
pub fn collect(socket: &UdpSocket, context: [u8; 8], window: Duration) -> io::Result<Sweep> {
    let deadline = Instant::now() + window;
    let mut buf = vec![0u8; 65535];
    let mut out = Sweep::default();
    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        socket.set_read_timeout(Some(deadline - now))?;
        let (n, from) = match socket.recv_from(&mut buf) {
            Ok(x) => x,
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => break,
            Err(e) => return Err(e),
        };
        match classify(&buf[..n], context) {
            Ok(items) => out
                .found
                .extend(items.into_iter().map(|identity| Found { from, identity })),
            Err(r) => out.rejected.push((from, r)),
        }
    }
    Ok(out)
}

/// Decode one datagram as a reply to our request.
///
/// # Errors
/// See [`Rejected`].
pub fn classify(datagram: &[u8], context: [u8; 8]) -> Result<Vec<IdentityItem>, Rejected> {
    let (frame, _rest) = Frame::decode(datagram).map_err(Rejected::BadFrame)?;
    if frame.header.command != Command::ListIdentity as u16 || frame.header.sender_context != context {
        return Err(Rejected::NotOurs);
    }
    decode_list_identity(frame.payload).map_err(Rejected::BadPayload)
}

/// A per-sweep sender context so stray replies from an earlier sweep are
/// filtered out. Derived from the clock, not randomness, so tests that need
/// a fixed context can call [`collect`] directly.
fn sender_context() -> [u8; 8] {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    u64::try_from(nanos & u128::from(u64::MAX))
        .unwrap_or(u64::MAX)
        .to_le_bytes()
}
