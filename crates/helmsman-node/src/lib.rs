//! The edge node's firmware library (docs/EDGE.md): what the two binaries
//! share. `bench` (Phase 1) needs none of it; `modbus` (Phase 2, the `net`
//! feature) brings the POE2's board bring-up, the Ethernet stack and the
//! Modbus TCP client over it.

#![no_std]

extern crate alloc;

#[cfg(feature = "net")]
pub mod board;
#[cfg(feature = "net")]
pub mod ethernet;
#[cfg(feature = "net")]
pub mod modbus;
