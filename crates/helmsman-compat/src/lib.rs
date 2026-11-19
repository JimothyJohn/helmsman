//! The release-train check (docs/COMPAT.md). Four repositories move on
//! their own `dev` branches and are promoted by hand: helmsman (the box
//! and the node firmware), agent-relay (the relay the box's agent talks
//! to), helmsman-devices (the device library a box fetches) and
//! helmsman-hardware (the node board). Each has its own gate; none of
//! them sees the others. This crate holds each contract between them,
//! black-box where the other side is a program, so that a box built at
//! an old ref (a `master` 300 commits behind `dev`) is checked by the
//! same code as one built at the tip:
//!
//! - [`devices`]: a box at one ref, pointed at a devices checkout at
//!   another, lists every library entry and reads its map, profile and
//!   record.
//! - [`relay`]: a box at one ref, a relay at another, a stub model behind
//!   it: every profile the box asks for by default is served, and a chat,
//!   help and design turn completes with every offered tool known to the
//!   box. `--live` repeats the profile lookups and one chat turn against
//!   the deployed relay.
//! - [`hardware`]: the node firmware's pin wiring against the board's
//!   netlist.
//!
//! `scripts/compat.sh` checks the repositories out at the refs, builds
//! the programs and runs each contract on its three corners (both `dev`,
//! and each `dev` against the other's live default branch).

pub mod boxp;
pub mod client;
pub mod devices;
pub mod hardware;
pub mod relay;
pub mod report;
pub mod web;
