//! The edge node's core, with nothing in it that touches a socket, a
//! clock or flash: what a node stores ([`settings`]), how it reads a
//! request and writes a reply ([`http`]), what it says about itself in
//! the box API's shapes ([`api`]), and which route does what
//! ([`router`]). The firmware (crates/helmsman-node, crates/helmsman-node-p4)
//! moves the bytes and carries out the [`router::Effect`]s; the canvas
//! parses the JSON here exactly as it parses a box's, which its tests
//! hold it to.
//!
//! Why a node speaks the box API: the canvas already knows how to watch
//! a box (`GET /api/modules`, `/api/events`, `/api/devices`,
//! `/api/io_map`, a module's source), and a node that answers those from
//! its own port is a box to it, with no second kind of thing to maintain
//! (Nick, 2026-11-08). What a node does not have (traces, proposals,
//! snapshots, accounts) answers 404 with a reason.
//!
//! Trust: reads are open on the LAN, as a box's are today. Anything that
//! changes the node (a deploy, a module control, the settings) needs
//! `Authorization: Bearer <token>`, the token from the settings record;
//! there are no sessions and no accounts on a node.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod api;
pub mod http;
pub mod router;
pub mod settings;

/// The crate's version, which the node reports as its firmware version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
