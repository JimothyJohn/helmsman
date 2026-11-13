//! Configuration of third-party devices over their own configuration
//! protocols (docs/ARCHITECTURE.md, "Device configuration").
//!
//! The first device is the Moxa ioLogik E1200 web console
//! ([`moxa_e1200`]). Std only: no dependency, no `unsafe`.

#![forbid(unsafe_code)]

pub mod md5;
pub mod moxa_e1200;
