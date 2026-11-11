//! PROFINET IO, RT class 1, as bytes and as two state machines without
//! I/O. `no_std + alloc`, no dependencies.
//!
//! - [`eth`], [`dcp`], [`rpc`], [`block`]: the frames and PDUs.
//!
//! References (docs/SOURCES.md, "PROFINET IO, by secondary references"):
//! IEC 61158-5-10 / -6-10 are not held. Layouts are taken from p-net
//! (rtlabs-com/p-net, an IO device stack) and Wireshark's PROFINET
//! dissectors, cited by file and function, and the RPC envelope from DCE
//! 1.1: RPC (Open Group C706).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod alarm;
pub mod block;
pub mod codec;
pub mod dcp;
pub mod eth;
pub mod rpc;
pub mod rt;

pub use codec::DecodeError;
