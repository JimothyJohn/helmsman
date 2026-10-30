//! Target (adapter) side of CIP over TCP/UDP on Ethernet.
//!
//! [`device::Device`] is the protocol state machine with no sockets and no
//! clock of its own: every call takes `now`, so behaviour (timeouts,
//! production schedule) is testable deterministically. [`server`] puts it
//! on the network with std threads.
//!
//! Behaviour follows `OpENer`'s target where the specification text is not
//! in this repo: request layouts, refusal codes and their order, sizes with
//! and without the run/idle header, and where T->O data is sent. Module docs
//! cite the `OpENer` source each rule comes from.
//!
//! Not an ODVA product; conveys no licence to the EtherNet/IP technology.

#![forbid(unsafe_code)]

pub mod device;
pub mod logix;
pub mod server;

pub use device::{ConnectionPoint, Device, DeviceConfig, Identity, PointKind};
pub use server::{Adapter, ServerConfig};
