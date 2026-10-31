//! I/O mapping for the box. The box is the PLC: it is the master (client) of
//! the devices it talks to, and its logic only ever names *signals*:
//!
//! - a [`Device`] is an endpoint the box masters, spoken to by a driver
//!   (`modbus-tcp`, `modbus-rtu` on a serial line ([`rtu`]), [`enip`]
//!   (`enip-io`, an EtherNet/IP adapter over a Class 1 connection), and
//!   [`logix`] read-only for controllers the box only observes;
//!   and drivers compiled to WebAssembly ([`wasm`], the `wasm` feature),
//!   which reach only their own device;
//! - a [`Signal`] is a named, typed point with a direction, bound to a point on
//!   a device or to one of the box's own channels (a GPIO driving a 24 V
//!   output stage, say);
//! - a [`Driver`] reads and writes points by address string, the same
//!   contract a sandboxed driver will implement;
//! - [`local`] is the box's own channels (`do:N`, `di:N`, `i2c:...`) in
//!   memory, behind the same contract; [`board`] is the same channels on a
//!   Pi's GPIO lines and I2C buses (Linux, the `gpio` feature);
//! - [`harness`] checks a driver against a device, on the [`sim`] or on a
//!   bench, with cross-wired outputs proving a write physically landed.

#![forbid(unsafe_code)]

pub mod board;
pub mod driver;
pub mod enip;
#[cfg(feature = "enip-sim")]
pub mod enip_sim;
pub mod harness;
pub mod local;
pub mod logix;
pub mod modbus;
pub mod model;
pub mod rtu;
pub mod sim;
pub mod text;
#[cfg(feature = "wasm")]
pub mod wasm;

pub use driver::{Driver, Error, PointInfo, open, open_on};
pub use model::{
    Binding, Device, Dir, IoMap, MAX_POLL, MAX_UNIT, MapError, Signal, Ty, Value, module_name_ok, tile_ok,
    unit_ok,
};
