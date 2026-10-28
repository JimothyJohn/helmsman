//! Soft PLC built on the adapter: a scan engine running a small language
//! over the I/O assemblies, and a browser programming interface.

#![forbid(unsafe_code)]

pub mod engine;
pub mod json;
pub mod lang;
pub mod web;
