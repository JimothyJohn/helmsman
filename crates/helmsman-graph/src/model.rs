//! The data: what is on the canvas, saved as JSON.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A node within one graph. Ids are local to their graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeId(pub u32);

/// A function block definition within a project.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BlockId(pub u32);

/// Port type. Wires only join ports of the same type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Ty {
    Bool,
    Num,
}

/// A value on a wire.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Bool(bool),
    Num(f64),
}

impl Value {
    #[must_use]
    pub fn zero(ty: Ty) -> Self {
        match ty {
            Ty::Bool => Self::Bool(false),
            Ty::Num => Self::Num(0.0),
        }
    }

    #[must_use]
    pub fn as_bool(self) -> bool {
        match self {
            Self::Bool(b) => b,
            Self::Num(n) => n != 0.0,
        }
    }

    #[must_use]
    pub fn as_num(self) -> f64 {
        match self {
            Self::Bool(b) => f64::from(u8::from(b)),
            Self::Num(n) => n,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MathOp {
    Add,
    Sub,
    Mul,
    Div,
    Min,
    Max,
}

impl MathOp {
    pub const ALL: [Self; 6] = [Self::Add, Self::Sub, Self::Mul, Self::Div, Self::Min, Self::Max];

    #[must_use]
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::Min => "MIN",
            Self::Max => "MAX",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CmpOp {
    Gt,
    Ge,
    Lt,
    Le,
    Eq,
    Ne,
}

impl CmpOp {
    pub const ALL: [Self; 6] = [Self::Gt, Self::Ge, Self::Lt, Self::Le, Self::Eq, Self::Ne];

    #[must_use]
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Eq => "=",
            Self::Ne => "<>",
        }
    }
}

/// What a node does. Parameters live here; runtime state lives in the
/// simulator.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Kind {
    /// Latching switch; click to flip.
    Toggle {
        on: bool,
    },
    /// Momentary push button; true while held.
    Button,
    Const {
        value: f64,
    },
    /// Square wave: true for the first half of each period.
    Clock {
        period_ms: f64,
    },
    Lamp,
    Display,
    And {
        inputs: u8,
    },
    Or {
        inputs: u8,
    },
    Xor,
    Not,
    /// Set-dominant latch.
    Sr,
    /// Reset-dominant latch.
    Rs,
    RTrig,
    FTrig,
    Ton {
        preset_ms: f64,
    },
    Tof {
        preset_ms: f64,
    },
    Tp {
        preset_ms: f64,
    },
    /// Retentive on-delay with reset (Rockwell's RTO).
    Tonr {
        preset_ms: f64,
    },
    /// Up counter with reset.
    Ctu {
        preset: f64,
    },
    Math(MathOp),
    Compare(CmpOp),
    /// `G ? IN1 : IN0`.
    Select,
    /// A function block's input, seen from inside it.
    PinIn {
        name: String,
        ty: Ty,
    },
    /// A function block's output, seen from inside it.
    PinOut {
        name: String,
        ty: Ty,
    },
    /// An instance of a function block.
    Block(BlockId),
    /// Sticky note; no ports, never simulated.
    Note {
        text: String,
    },
}

/// Gate inputs are clamped to this range.
pub const GATE_INPUTS: std::ops::RangeInclusive<u8> = 2..=8;

impl Kind {
    /// Title shown on the node when it has no label.
    #[must_use]
    pub fn title(&self, project: &Project) -> String {
        match self {
            Self::Toggle { .. } => "Toggle".into(),
            Self::Button => "Button".into(),
            Self::Const { .. } => "Const".into(),
            Self::Clock { .. } => "Clock".into(),
            Self::Lamp => "Lamp".into(),
            Self::Display => "Display".into(),
            Self::And { .. } => "AND".into(),
            Self::Or { .. } => "OR".into(),
            Self::Xor => "XOR".into(),
            Self::Not => "NOT".into(),
            Self::Sr => "SR".into(),
            Self::Rs => "RS".into(),
            Self::RTrig => "R_TRIG".into(),
            Self::FTrig => "F_TRIG".into(),
            Self::Ton { .. } => "TON".into(),
            Self::Tof { .. } => "TOF".into(),
            Self::Tp { .. } => "TP".into(),
            Self::Tonr { .. } => "TONR".into(),
            Self::Ctu { .. } => "CTU".into(),
            Self::Math(op) => op.symbol().into(),
            Self::Compare(op) => op.symbol().into(),
            Self::Select => "SEL".into(),
            Self::PinIn { name, .. } | Self::PinOut { name, .. } => name.clone(),
            Self::Block(b) => project
                .blocks
                .get(b)
                .map_or_else(|| "missing block".into(), |d| d.name.clone()),
            Self::Note { .. } => "Note".into(),
        }
    }

    /// The palette: every kind a user can add, with default parameters, under
    /// a category. Pins and blocks are added separately.
    #[must_use]
    pub fn palette() -> Vec<(&'static str, Self)> {
        vec![
            ("Inputs", Self::Toggle { on: false }),
            ("Inputs", Self::Button),
            ("Inputs", Self::Const { value: 1.0 }),
            ("Inputs", Self::Clock { period_ms: 1000.0 }),
            ("Outputs", Self::Lamp),
            ("Outputs", Self::Display),
            ("Logic", Self::And { inputs: 2 }),
            ("Logic", Self::Or { inputs: 2 }),
            ("Logic", Self::Xor),
            ("Logic", Self::Not),
            ("Latches", Self::Sr),
            ("Latches", Self::Rs),
            ("Latches", Self::RTrig),
            ("Latches", Self::FTrig),
            ("Timers", Self::Ton { preset_ms: 1000.0 }),
            ("Timers", Self::Tof { preset_ms: 1000.0 }),
            ("Timers", Self::Tp { preset_ms: 1000.0 }),
            ("Timers", Self::Tonr { preset_ms: 1000.0 }),
            ("Counters", Self::Ctu { preset: 3.0 }),
            ("Math", Self::Math(MathOp::Add)),
            ("Math", Self::Compare(CmpOp::Gt)),
            ("Math", Self::Select),
            ("Notes", Self::Note { text: "note".into() }),
        ]
    }
}

/// A port on a node. `key` is what wires refer to: the position for built-in
/// kinds, the pin's node id for a block instance (so reordering pins inside
/// the definition never rewires its instances).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Port {
    pub key: u32,
    pub name: String,
    pub ty: Ty,
}

fn fixed(ports: &[(&str, Ty)]) -> Vec<Port> {
    (0u32..)
        .zip(ports)
        .map(|(key, &(name, ty))| Port {
            key,
            name: name.into(),
            ty,
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub kind: Kind,
    /// Top-left corner in canvas units.
    pub pos: [f32; 2],
    /// Shown instead of the kind's title when not empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
}

impl Node {
    #[must_use]
    pub fn new(kind: Kind, pos: [f32; 2]) -> Self {
        Self {
            kind,
            pos,
            label: String::new(),
        }
    }

    #[must_use]
    pub fn title(&self, project: &Project) -> String {
        if self.label.is_empty() {
            self.kind.title(project)
        } else {
            self.label.clone()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Wire {
    pub from: NodeId,
    pub from_port: u32,
    pub to: NodeId,
    pub to_port: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Graph {
    pub nodes: BTreeMap<NodeId, Node>,
    pub wires: Vec<Wire>,
    pub next_id: u32,
}

impl Graph {
    /// Adds a node without any checks; see [`Project::add_node`].
    pub fn insert(&mut self, node: Node) -> NodeId {
        let floor = self.nodes.keys().next_back().map_or(0, |id| id.0 + 1);
        let id = NodeId(self.next_id.max(floor));
        self.next_id = id.0 + 1;
        self.nodes.insert(id, node);
        id
    }

    /// The wire feeding an input, if any.
    #[must_use]
    pub fn wire_into(&self, to: NodeId, to_port: u32) -> Option<&Wire> {
        self.wires.iter().find(|w| w.to == to && w.to_port == to_port)
    }

    /// Pins of this graph seen as a block interface, in display order (top to
    /// bottom on the canvas, ties by id).
    #[must_use]
    pub fn pins(&self, out: bool) -> Vec<Port> {
        let mut pins: Vec<(f32, NodeId, Port)> = self
            .nodes
            .iter()
            .filter_map(|(id, n)| match (&n.kind, out) {
                (Kind::PinIn { name, ty }, false) | (Kind::PinOut { name, ty }, true) => Some((
                    n.pos[1],
                    *id,
                    Port {
                        key: id.0,
                        name: name.clone(),
                        ty: *ty,
                    },
                )),
                _ => None,
            })
            .collect();
        pins.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        pins.into_iter().map(|(_, _, p)| p).collect()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockDef {
    pub name: String,
    pub graph: Graph,
}

/// Which graph an edit applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    Root,
    Block(BlockId),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub root: Graph,
    pub blocks: BTreeMap<BlockId, BlockDef>,
    pub next_block: u32,
}

impl Project {
    #[must_use]
    pub fn graph(&self, scope: Scope) -> Option<&Graph> {
        match scope {
            Scope::Root => Some(&self.root),
            Scope::Block(b) => self.blocks.get(&b).map(|d| &d.graph),
        }
    }

    pub fn graph_mut(&mut self, scope: Scope) -> Option<&mut Graph> {
        match scope {
            Scope::Root => Some(&mut self.root),
            Scope::Block(b) => self.blocks.get_mut(&b).map(|d| &mut d.graph),
        }
    }

    /// Input and output ports of a node.
    #[must_use]
    pub fn ports(&self, kind: &Kind) -> (Vec<Port>, Vec<Port>) {
        use Ty::{Bool as B, Num as N};
        match kind {
            Kind::Toggle { .. } | Kind::Button | Kind::Clock { .. } => (vec![], fixed(&[("Q", B)])),
            Kind::Const { .. } => (vec![], fixed(&[("OUT", N)])),
            Kind::Lamp => (fixed(&[("IN", B)]), vec![]),
            Kind::Display => (fixed(&[("IN", N)]), vec![]),
            Kind::And { inputs } | Kind::Or { inputs } => {
                let n = (*inputs).clamp(*GATE_INPUTS.start(), *GATE_INPUTS.end());
                let ins = (1..=n)
                    .map(|i| Port {
                        key: u32::from(i - 1),
                        name: format!("IN{i}"),
                        ty: B,
                    })
                    .collect();
                (ins, fixed(&[("Q", B)]))
            }
            Kind::Xor => (fixed(&[("IN1", B), ("IN2", B)]), fixed(&[("Q", B)])),
            Kind::Not => (fixed(&[("IN", B)]), fixed(&[("Q", B)])),
            Kind::Sr | Kind::Rs => (fixed(&[("S", B), ("R", B)]), fixed(&[("Q", B)])),
            Kind::RTrig | Kind::FTrig => (fixed(&[("CLK", B)]), fixed(&[("Q", B)])),
            Kind::Ton { .. } | Kind::Tof { .. } | Kind::Tp { .. } => {
                (fixed(&[("IN", B)]), fixed(&[("Q", B), ("ET", N)]))
            }
            Kind::Tonr { .. } => (fixed(&[("IN", B), ("R", B)]), fixed(&[("Q", B), ("ET", N)])),
            Kind::Ctu { .. } => (fixed(&[("CU", B), ("R", B)]), fixed(&[("Q", B), ("CV", N)])),
            Kind::Math(_) => (fixed(&[("A", N), ("B", N)]), fixed(&[("OUT", N)])),
            Kind::Compare(_) => (fixed(&[("A", N), ("B", N)]), fixed(&[("Q", B)])),
            Kind::Select => (fixed(&[("G", B), ("IN0", N), ("IN1", N)]), fixed(&[("OUT", N)])),
            Kind::PinIn { ty, .. } => (vec![], fixed(&[("", *ty)])),
            Kind::PinOut { ty, .. } => (fixed(&[("", *ty)]), vec![]),
            Kind::Block(b) => self
                .blocks
                .get(b)
                .map_or((vec![], vec![]), |d| (d.graph.pins(false), d.graph.pins(true))),
            Kind::Note { .. } => (vec![], vec![]),
        }
    }

    /// Serialized form, as saved and exported.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a project always serializes")
    }

    /// Parses a saved project and repairs what a hand edit could break:
    /// dangling or mistyped wires are dropped, and a block that contains
    /// itself (directly or through others) is refused.
    ///
    /// # Errors
    /// Malformed JSON, or a block nested inside itself.
    pub fn from_json(s: &str) -> Result<Self, String> {
        let mut p: Self = serde_json::from_str(s).map_err(|e| e.to_string())?;
        for &b in p.blocks.keys() {
            if p.reaches(b, b) {
                return Err(format!("block {:?} contains itself", p.blocks[&b].name));
            }
        }
        p.prune();
        Ok(p)
    }

    /// Whether an instance of `from` would, through any depth of nesting,
    /// contain an instance of `target` (`from == target` counts only when the
    /// definition contains itself somewhere).
    #[must_use]
    pub fn reaches(&self, from: BlockId, target: BlockId) -> bool {
        let mut stack = vec![from];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(b) = stack.pop() {
            if !seen.insert(b) {
                continue;
            }
            let Some(def) = self.blocks.get(&b) else { continue };
            for n in def.graph.nodes.values() {
                if let Kind::Block(c) = n.kind {
                    if c == target {
                        return true;
                    }
                    stack.push(c);
                }
            }
        }
        false
    }

    /// Number of instances of a block across every graph.
    #[must_use]
    pub fn uses(&self, block: BlockId) -> usize {
        std::iter::once(&self.root)
            .chain(self.blocks.values().map(|d| &d.graph))
            .flat_map(|g| g.nodes.values())
            .filter(|n| n.kind == Kind::Block(block))
            .count()
    }
}
