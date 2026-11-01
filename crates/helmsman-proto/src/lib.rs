//! Originator side of CIP over TCP/UDP on Ethernet: discover devices,
//! send explicit messages, and (later) run Class 1 I/O connections.
//!
//! Wire-format references, cited per item in the module docs:
//! - ODVA PUB00213R0 (Quick Start for Vendors Handbook), glossary pp.23–26.
//! - `OpENer` (`EIPStackGroup/OpENer`) `source/src/enet_encap/{encap,cpf,endianconv}.c`,
//!   used as the reference adapter when the specification text itself is not
//!   available in this repo.
//!
//! # Read-only by default
//!
//! Without the `unrestricted` feature the only ways to put a request on the
//! wire are reads: [`session::Session::get_attribute_single`] and
//! [`logix::TagReader`] (Read Tag, Multiple Service Packet of reads, Read
//! Tag Fragmented, Symbol object browse). [`session::Session::request`] /
//! `request_raw` (any service) exist only with `unrestricted`. The box
//! build (`helmsman-plc`) must not enable it; the tests do. The
//! byte codecs stay public either way: the adapter needs them to answer
//! requests, and they open no socket.
//!
//! `scanner` (Class 1 connections to a device's assemblies, which drive
//! its outputs) is its own feature, which `unrestricted` implies. It can
//! send `Forward_Open` and `Forward_Close` with an Assembly-class path and
//! nothing else, so the box build may enable it for devices it masters
//! (`helmsman-io`'s `enip-io` driver) without gaining any other service.
//!
//! [`eds`] reads Electronic Data Sheets: text, no sockets, no feature.
//!
//! This crate is not an ODVA product and conveys no license to the
//! EtherNet/IP technology or marks.

#![forbid(unsafe_code)]

pub mod cip;
pub mod codec;
pub mod cpf;
pub mod discover;
pub mod eds;
pub mod encap;
pub mod endpoint;
pub mod explicit;
pub mod identity;
pub mod io;
pub mod logix;
#[cfg(feature = "scanner")]
pub mod scanner;
pub mod session;

pub use codec::DecodeError;
