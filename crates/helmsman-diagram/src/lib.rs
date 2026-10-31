//! A module's Structured Text as an FBD-like diagram the operator watches:
//! the canvas's read-only view of the code (`st-graph-view`). ST stays the
//! source of truth; nothing here writes it back.
//!
//! - [`lower()`]: each user POU of the source to a [`Diagram`]: blocks for
//!   operators, functions and function block calls, wires for data flow
//!   from variables to assignments, IF/CASE arms and loop bodies as gated
//!   [`Region`]s. Every node, region and statement carries the
//!   [`helmsman_iec::trace::StmtId`] it came from, so a scan trace maps
//!   straight onto it.
//! - [`layout`]: deterministic left-to-right placement, no UI needed.
//! - [`live::annotate`]: one traced scan on the diagram (values on wires,
//!   gates open or closed).
//! - [`explain()`]: "on because …" for any variable, as data.
//! - [`diff`]: what a proposal changes, line by line in the source and
//!   block by block in the diagram, for the review screen.
//!
//! ```
//! use std::time::Duration;
//! use helmsman_iec::{Image, ImageSizes, compile};
//!
//! let src = "PROGRAM p
//! VAR start, stop, run : BOOL; END_VAR
//! run := (start OR run) AND NOT stop;
//! END_PROGRAM";
//! let sizes = ImageSizes { i: 4, q: 4, m: 4 };
//! let mut prog = compile(src, sizes).unwrap();
//! let module = helmsman_diagram::lower(src, &prog.trace_map()).unwrap();
//! let d = &module.program().unwrap().diagram;
//! assert_eq!(d.nodes.len(), 7, "3 reads, OR, NOT, AND, the write");
//! let (_, trace) = prog.scan_traced(&mut Image::new(sizes), Duration::ZERO);
//! let why = helmsman_diagram::explain(&module, &trace, "run", 2);
//! assert!(why.outline()[0].starts_with("RUN = FALSE because line 3"));
//! ```

// Layout geometry is f32 throughout; counts convert into it. Lowering and
// layout read best with short names (n, e, s, r) and one arm per statement kind.
#![allow(
    clippy::cast_precision_loss,
    clippy::many_single_char_names,
    clippy::too_many_lines,
    clippy::single_match_else
)]

pub mod diff;
pub mod explain;
pub mod layout;
pub mod live;
pub mod lower;
pub mod model;
pub mod phrase;
pub mod text;

pub use explain::{Because, Why, explain};
pub use live::{Gate, Live, Scan, annotate};
pub use lower::{LowerError, Module, Pou, StmtPos, lower, lower_table};
pub use model::{Diagram, Node, NodeId, NodeKind, Rect, Region, RegionId, RegionKind, Stmt, Wire};
pub use phrase::{Phrased, Sentence, Words, phrase};
