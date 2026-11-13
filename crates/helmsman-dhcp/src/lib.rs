//! A DHCP (IPv4) and BOOTP server for the box's own device network (TODO.md
//! `box-dhcp-server`): blocks that ship as DHCP clients (Turck TBEN at
//! rotary position 600, Murrelektronik IMPACT67 E at 000) get an address
//! with nobody touching them. `no_std + alloc`, no dependencies.
//!
//! - [`message`]: the BOOTP/DHCP message as bytes (RFC 2131 section 2,
//!   RFC 2132 for the options, RFC 1542 for BOOTP's minimum size).
//! - [`server`]: the server as a state machine; every call takes `now`
//!   (seconds since the Unix epoch) and returns the reply to send, if any.
//! - [`leases`]: the lease file the box keeps in its state directory.
//! - [`guard`]: the check for another server: a probe DISCOVER, and
//!   whether a reply seen on the wire came from someone else.
//!
//! What it is not: a server for a network someone else runs. It answers
//! only clients on its own segment (a relayed request, `giaddr` set, is
//! dropped), only Ethernet clients, and the box starts it only on an
//! interface an engineer named, after [`guard`] has found no other server.
//!
//! References (docs/SOURCES.md, "DHCP and BOOTP"): RFC 2131, RFC 2132,
//! RFC 1542, RFC 951, RFC 3396 (a long option split in pieces).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod guard;
pub mod leases;
pub mod message;
pub mod server;

pub use message::{DecodeError, Mac, Message};
pub use server::{Config, Dest, Reply, Server};

/// UDP port a server listens on (BOOTPS).
pub const SERVER_PORT: u16 = 67;
/// UDP port a client listens on (BOOTPC).
pub const CLIENT_PORT: u16 = 68;
