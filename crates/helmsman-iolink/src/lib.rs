//! IO-Link (SDCI): the octets on the C/Q line, and a Master port and a
//! Device as state machines without I/O. `no_std`, no allocation, no
//! dependencies: every encoder writes into the caller's buffer, every
//! decoder borrows from it, and every call that depends on time takes
//! `now`.
//!
//! - [`checksum`]: the 6-bit message checksum.
//! - [`mseq`]: the M-sequence control octet, the M-sequence types as
//!   layouts, the Master message and the Device reply, and which type a
//!   Device's capability selects.
//! - [`page`]: Direct Parameter page 1 (the `MasterCommand`s, cycle times,
//!   the M-sequence capability, Process Data lengths, vendor and device
//!   id).
//! - [`timing`]: the transmission rates and the wake-up, retry and
//!   M-sequence times.
//! - [`master`]: one Master port, from the wake-up to cyclic Process
//!   Data, with retries and the loss of the Device.
//! - [`device`]: a simulated Device, for tests and benches.
//! - [`pd`]: values inside Process Data by bit offset and length.
//! - `iodd` (the `iodd` feature, which allocates): a Device's IODD file
//!   read for its identity and the layout of its Process Data.
//!
//! Reference (docs/SOURCES.md, "IO-Link"): IO-Link Interface and System
//! Specification V1.1.5 (IO-Link Community, order no. 10.002, October
//! 2025), cited as "IOL §x p.y".
//!
//! Not here yet: a transceiver driver (the caller moves the octets),
//! ISDUs (§A.5), Events (§A.6), the interleaved `TYPE_1_1` / `TYPE_1_2`
//! Process Data mode (§7.3.4.2), legacy (revision 1.0) Devices (Table
//! A.9), SIO mode and data storage.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

#[cfg(feature = "iodd")]
extern crate alloc;

pub mod checksum;
pub mod device;
#[cfg(feature = "iodd")]
pub mod iodd;
pub mod master;
pub mod mseq;
pub mod page;
pub mod pd;
pub mod timing;

pub use mseq::{Channel, Error, Layout, MasterMessage, Mc, Reply};
