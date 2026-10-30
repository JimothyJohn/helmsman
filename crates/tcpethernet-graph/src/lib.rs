//! Node graph model behind the canvas: nodes with typed ports, wires,
//! function blocks that nest other function blocks, and a scan-cycle
//! simulator. No UI and no I/O here; the canvas crate draws and edits a
//! [`Project`], [`Sim`] runs it.
//!
//! A function block is a [`BlockDef`]: a graph whose `PinIn`/`PinOut` nodes
//! are its interface. Every `Block` node is an instance of a definition, so
//! editing the definition changes every instance; each instance keeps its own
//! simulation state (timers, counters, latches).

mod edit;
mod model;
mod sim;

pub use edit::EditError;
pub use model::{
    BlockDef, BlockId, CmpOp, Graph, Kind, MathOp, Node, NodeId, Port, Project, Scope, Ty, Value, Wire,
};
pub use sim::{Inst, Mem, Sim};
