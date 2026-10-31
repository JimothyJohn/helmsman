//! Reading tags from a Logix-family controller (a `CompactLogix` or
//! `ControlLogix`) by name, over an unconnected explicit session. Read-only
//! by construction: see [`wire::ReadRequest`] and [`TagReader`]. There is
//! no Write Tag here, and no way to name an arbitrary service code.
//!
//! The reference for these services is the controller vendor's "Logix 5000
//! Controllers Data Access" programming manual (publication 1756-PM020); it
//! could not be fetched for this repo (HTTP 403), so the byte layouts come
//! from the open-source clients cited per module: pycomm3 (MIT) and
//! libplctag (MPL 2.0 / LGPL), read for layouts only. See `docs/SOURCES.md`.
//! Real-controller verification is still pending hardware.
//!
//! Not a product of, or endorsed by, the controller's vendor or ODVA.

pub mod path;
pub mod reader;
pub mod types;
pub mod wire;

pub use path::{Element, PathError, TagPath};
pub use reader::{Error, TagReader};
pub use types::{AtomicType, DataType, TagData, Value};
pub use wire::{ReadRequest, SYMBOL_CLASS, Symbol};
