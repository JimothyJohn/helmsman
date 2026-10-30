//! I/O mapping for the box. The box is the PLC: it is the master (client) of
//! the devices it talks to, and its logic only ever names *signals*:
//!
//! - a [`Device`] is an endpoint the box masters, spoken to by a driver
//!   (`modbus-tcp` today; agent-written WASM drivers later);
//! - a [`Signal`] is a named, typed point with a direction, bound to a point on
//!   a device or to one of the box's own channels (a GPIO driving a 24 V
//!   output stage, say);
//! - a [`Driver`] reads and writes points by address string, the same
//!   contract a sandboxed driver will implement;
//! - [`local`] is the box's own channels (`do:N`, `di:N`) behind the same
//!   contract; in memory for now, GPIO later;
//! - [`harness`] checks a driver against a device, on the [`sim`] or on a
//!   bench, with cross-wired outputs proving a write physically landed.

#![forbid(unsafe_code)]

pub mod driver;
pub mod harness;
pub mod local;
pub mod modbus;
pub mod model;
pub mod sim;

pub use driver::{Driver, Error, PointInfo, open};
pub use model::{Binding, Device, Dir, IoMap, MapError, Signal, Ty, Value};
