//! The ESP32-P4 edge node's firmware library (docs/EDGE.md, Phase 3): what
//! the binaries share. The board (`board`), the Ethernet bring-up and the
//! embassy-net stack over esp-hal's EMAC driver (`ethernet`), a Modbus TCP
//! client (`modbus`), an EtherNet/IP originator (`enip`: the explicit
//! session for `Forward_Open`, and the Class 1 I/O connection around
//! helmsman-proto's `class1`), and the UDP report line (`report`).
//!
//! Next door, crates/helmsman-node is the same idea on the Xtensa
//! ESP32-POE2 with its own toolchain; this crate is the one that runs on
//! upstream stable Rust.
#![no_std]

extern crate alloc;

pub mod beacon;
pub mod board;
pub mod enip;
pub mod ethernet;
pub mod flash;
pub mod io;
pub mod modbus;
pub mod report;
pub mod server;
pub mod state;
