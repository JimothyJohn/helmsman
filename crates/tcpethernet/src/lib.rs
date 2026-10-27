//! Originator side of CIP over TCP/UDP on Ethernet: discover devices,
//! send explicit messages, and (later) run Class 1 I/O connections.
//!
//! Wire-format references, cited per item in the module docs:
//! - ODVA PUB00213R0 (Quick Start for Vendors Handbook), glossary pp.23–26.
//! - `OpENer` (`EIPStackGroup/OpENer`) `source/src/enet_encap/{encap,cpf,endianconv}.c`,
//!   used as the reference adapter when the specification text itself is not
//!   available in this repo.
//!
//! This crate is not an ODVA product and conveys no license to the
//! EtherNet/IP technology or marks.

#![forbid(unsafe_code)]

pub mod cip;
pub mod codec;
pub mod cpf;
pub mod discover;
pub mod encap;
pub mod explicit;
pub mod identity;
pub mod session;

pub use codec::DecodeError;
