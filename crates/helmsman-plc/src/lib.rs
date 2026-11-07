//! The box's runtime, as a library: what the `helmsman-plc` binary runs
//! behind the CIP adapter, testable without sockets or a clock.

#![forbid(unsafe_code)]

#[cfg(feature = "box")]
pub mod api;
#[cfg(feature = "box")]
pub mod bundle;
pub mod calendar;
#[cfg(feature = "box")]
pub mod driver_dir;
pub mod drivers;
pub mod fallback;
pub mod force;
pub mod layout;
pub mod link;
pub mod logic;
pub mod manifest;
#[cfg(feature = "box")]
pub mod opcua;
#[cfg(feature = "box")]
pub mod policy;
#[cfg(feature = "box")]
pub mod project;
pub mod record;
pub mod replay;
pub mod runtime;
pub mod signals;
pub mod sink;
#[cfg(feature = "box")]
pub mod site;
pub mod spec;
#[cfg(feature = "box")]
pub mod store;
pub mod tap;
#[cfg(feature = "wasm")]
pub mod wasm;

pub use logic::Logic;
pub use runtime::{ConfigError, MAX_PERIOD, Module, Runtime, Stats, Status, Window};
pub use signals::{BindError, SignalTable, TableError};
pub use tap::{Sample, TraceTap};

/// This build's stamp: the checkout's `git describe` when it was built
/// (`crates/build_stamp.rs`), or "unknown". The API sends it on every reply
/// as [`BUILD_HEADER`]; the canvas it serves carries its own and says when
/// the two differ.
pub const BUILD: &str = env!("HELMSMAN_BUILD");

/// The response header that carries [`BUILD`].
pub const BUILD_HEADER: &str = "X-Helmsman-Build";
