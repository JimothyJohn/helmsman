//! The box's runtime, as a library: what the `tcpethernet-plc` binary runs
//! behind the CIP adapter, testable without sockets or a clock.

#![forbid(unsafe_code)]

pub mod link;
pub mod runtime;
pub mod signals;
pub mod spec;

pub use runtime::{ConfigError, MAX_PERIOD, Module, Runtime, Stats, Status, Window};
pub use signals::{BindError, SignalTable, TableError};
