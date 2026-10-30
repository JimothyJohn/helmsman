//! The box's runtime, as a library: what the `helmsman-plc` binary runs
//! behind the CIP adapter, testable without sockets or a clock.

#![forbid(unsafe_code)]

pub mod api;
pub mod calendar;
pub mod link;
pub mod manifest;
pub mod policy;
pub mod record;
pub mod replay;
pub mod runtime;
pub mod signals;
pub mod sink;
pub mod spec;
pub mod store;
pub mod tap;

pub use runtime::{ConfigError, MAX_PERIOD, Module, Runtime, Stats, Status, Window};
pub use signals::{BindError, SignalTable, TableError};
pub use tap::{Sample, TraceTap};
