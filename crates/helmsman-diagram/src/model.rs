//! What a diagram is made of: nodes with ports, wires between them, gated
//! regions for IF/CASE arms and loop bodies, and the statement list that
//! ties each back to the source.

use helmsman_iec::Value;
use helmsman_iec::ast::Pos;
use helmsman_iec::trace::StmtId;

/// A node: an index into [`Diagram::nodes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub u32);

/// A region: an index into [`Diagram::regions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegionId(pub u32);

/// An axis-aligned box in diagram units (about a pixel at zoom 1).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    #[must_use]
    pub const fn right(&self) -> f32 {
        self.x + self.w
    }

    #[must_use]
    pub const fn bottom(&self) -> f32 {
        self.y + self.h
    }

    #[must_use]
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.right() && y >= self.y && y <= self.bottom()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    /// Reads a variable: `start`, `t.ET`, `grid[1,2]`, `%IX0.0`. A variable
    /// already written earlier on the same path is wired from that write
    /// instead.
    Read { path: String },
    /// A literal (or an enumeration value); its value when it has a plain one.
    Const { value: Option<Value> },
    /// An operator: `AND`, `NOT`, `+`, `>=`, ...
    Op { op: String },
    /// A function: `ABS`, `SEL`, `INT_TO_REAL`, a user FUNCTION.
    Func { name: String },
    /// A function block instance called: its inputs as written, its outputs
    /// as the POU reads them (`t.Q`) or assigns them (`Q => x`).
    Fb {
        instance: String,
        /// Its type, upper-cased (`TON`, `MOTOR`); empty if not resolvable.
        fb: String,
    },
    /// An element or member read through a computed index (`a[i]`); its
    /// inputs are the index expressions.
    Access { path: String },
    /// Assigns a variable: `path :=`. Its output is the value stored. A
    /// computed index (`a[i] :=`) adds the index expressions as inputs.
    Write { path: String },
    /// A FOR loop's head: `FROM`, `TO`, `BY` in, the loop variable out.
    For { var: String },
    /// `EXIT`, `CONTINUE` or `RETURN`.
    Jump,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    /// What the block shows in its header.
    pub label: String,
    /// The source text this node computes (`start AND NOT stop`, `lamp := m.on`).
    pub text: String,
    /// Input port names, in order.
    pub ins: Vec<String>,
    /// Output port names, in order.
    pub outs: Vec<String>,
    /// The statement this node was lowered from.
    pub stmt: StmtId,
    /// Where its expression (or statement) starts in the source.
    pub pos: Pos,
    /// The innermost region it sits in; `None` at the top of the POU.
    pub region: Option<RegionId>,
    pub rect: Rect,
}

/// Data flow: output `out` of `from` into input `inp` of `to`.
///
/// `negated`: the FBD negation bubble on the input pin. The source's
/// output keeps its own value (the wire shows it); the consumer sees its
/// inverse. Only BOOL wires are negated (see [`crate::lower`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wire {
    pub from: NodeId,
    pub out: usize,
    pub to: NodeId,
    pub inp: usize,
    pub negated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionKind {
    If,
    Elsif,
    Else,
    Case,
    CaseElse,
    For,
    While,
    Repeat,
}

/// Statements that run only when a gate lets them: one arm of an IF or
/// CASE, or a loop body.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub id: RegionId,
    pub kind: RegionKind,
    /// The IF, CASE or loop statement.
    pub stmt: StmtId,
    /// Arm index as [`helmsman_iec::trace::Trace::arm`] counts it (0 for a loop).
    pub arm: usize,
    pub parent: Option<RegionId>,
    /// `IF start AND NOT stop`, `ELSE`, `CASE 1, 3..5`, `FOR i := 0 TO 9`.
    pub label: String,
    /// The node output gating it: the arm's condition, the CASE selector,
    /// the WHILE or UNTIL condition, the FOR head. `None` for an ELSE.
    pub gate: Option<(NodeId, usize)>,
    /// The gate is the inverse of that output (`IF NOT jam`): a negation
    /// bubble where the gate's wire enters the region.
    pub negated: bool,
    pub pos: Pos,
    /// Its statements, indices into [`Diagram::stmts`].
    pub body: Vec<usize>,
    pub rect: Rect,
}

/// One source statement and what it was lowered to.
#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub id: StmtId,
    pub pos: Pos,
    pub region: Option<RegionId>,
    /// The nodes the statement ends in (its writes, its call, its jump),
    /// or for an IF/CASE/loop the nodes of its conditions and head.
    pub roots: Vec<NodeId>,
    /// An IF's, CASE's or loop's regions, in arm order.
    pub arms: Vec<RegionId>,
}

/// One POU's body as a diagram.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Diagram {
    /// The POU, upper-cased.
    pub pou: String,
    pub nodes: Vec<Node>,
    pub wires: Vec<Wire>,
    pub regions: Vec<Region>,
    /// Every statement, in source order.
    pub stmts: Vec<Stmt>,
    /// The statements at the top of the body.
    pub top: Vec<usize>,
    /// Width and height of the laid-out diagram.
    pub size: (f32, f32),
}

impl Diagram {
    #[must_use]
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.0 as usize]
    }

    #[must_use]
    pub fn region(&self, id: RegionId) -> &Region {
        &self.regions[id.0 as usize]
    }

    /// Wires into `to`, in input order.
    pub fn inputs(&self, to: NodeId) -> impl Iterator<Item = &Wire> {
        let mut w: Vec<&Wire> = self.wires.iter().filter(|w| w.to == to).collect();
        w.sort_by_key(|w| w.inp);
        w.into_iter()
    }

    /// The regions around `region`, innermost first.
    pub fn chain(&self, region: Option<RegionId>) -> impl Iterator<Item = &Region> {
        std::iter::successors(region.map(|r| self.region(r)), |r| {
            r.parent.map(|p| self.region(p))
        })
    }

    /// The topmost node at `(x, y)`, if any.
    #[must_use]
    pub fn node_at(&self, x: f32, y: f32) -> Option<NodeId> {
        self.nodes
            .iter()
            .rev()
            .find(|n| n.rect.contains(x, y))
            .map(|n| n.id)
    }

    /// The innermost region at `(x, y)`, if any.
    #[must_use]
    pub fn region_at(&self, x: f32, y: f32) -> Option<RegionId> {
        // Children come after their parents.
        self.regions
            .iter()
            .rev()
            .find(|r| r.rect.contains(x, y))
            .map(|r| r.id)
    }
}
