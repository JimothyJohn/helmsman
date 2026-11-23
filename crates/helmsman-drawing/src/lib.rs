//! Parts out of a technical drawing, matched to the device library.
//!
//! A panel layout is an ASCII DXF file: pairs of lines, a numeric group
//! code and a value. This crate reads the subset that says what stands
//! where ([`dxf`]), lists the parts a drawing inserts with their tag, part
//! number, position and rotation ([`parts`]), and matches each to a record
//! of the device library as [`Match::Matched`], [`Match::Ambiguous`] or
//! [`Match::Unknown`] ([`matching`]).
//!
//! No dependencies and no I/O: the DXF comes in as bytes and the library as
//! a slice of [`LibraryRecord`] (the caller builds it from the index it
//! already holds, `helmsman_project::devlib`), so the crate builds for
//! `wasm32-unknown-unknown` and runs the same in the box and the browser.
//!
//! Nothing is skipped silently. Binary DXF is refused as
//! [`DxfError::Binary`]; an entity type or section this reader does not
//! read is named in [`Drawing::unsupported`] and
//! [`Drawing::sections_not_read`]; malformed or truncated input is a
//! [`DxfError`], never a panic. A model is never guessed: anything short of
//! an exact hit on an identifier of a record is `Ambiguous` or `Unknown`.
#![forbid(unsafe_code)]

pub mod dxf;
pub mod matching;
pub mod parts;

pub use dxf::{Drawing, DxfError, read};
pub use matching::{LibraryRecord, Match, Matcher};
pub use parts::{Part, Parts, parts};
