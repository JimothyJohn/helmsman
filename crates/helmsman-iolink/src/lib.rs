//! IO-Link (SDCI), the octets on the C/Q line and nothing that moves them.
//! `no_std`, no allocation, no dependencies: every encoder writes into the
//! caller's buffer and every decoder borrows from it.
//!
//! - [`checksum`]: the 6-bit message checksum.
//! - [`mseq`]: the M-sequence control octet, the M-sequence types as
//!   layouts, the Master message and the Device reply, and which type a
//!   Device's capability selects.
//! - [`page`]: Direct Parameter page 1 (the `MasterCommand`s, cycle times,
//!   the M-sequence capability, Process Data lengths, vendor and device
//!   id).
//!
//! Reference (docs/SOURCES.md, "IO-Link"): IO-Link Interface and System
//! Specification V1.1.5 (IO-Link Community, order no. 10.002, October
//! 2025), cited as "IOL §x p.y".
//!
//! Not here yet: the wake-up and the UART timing (IOL §5.3.3, §A.3), the
//! Master and Device state machines, ISDUs (§A.5), Events (§A.6), the
//! interleaved `TYPE_1_1` / `TYPE_1_2` Process Data mode (§7.3.4.2) and
//! legacy (revision 1.0) Devices (Table A.9).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod checksum;
pub mod mseq;
pub mod page;

pub use mseq::{Channel, Error, Layout, MasterMessage, Mc, Reply};
