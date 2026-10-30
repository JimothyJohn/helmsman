//! Edits that keep a project consistent: typed wiring, no block nested inside
//! itself, and collapsing a selection into a new function block (and back).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::model::{BlockDef, BlockId, Graph, Kind, Node, NodeId, Project, Scope, Ty, Wire};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    NoSuchGraph,
    NoSuchNode,
    NoSuchPort,
    /// Output-to-output, input-to-input, or wrong direction.
    Direction,
    TypeMismatch {
        from: Ty,
        to: Ty,
    },
    /// The block would end up inside itself.
    Recursive,
    /// Pins belong to the block they are in; they cannot move into another.
    PinInSelection,
    PinAtRoot,
    EmptySelection,
    NotABlock,
    InUse(usize),
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchGraph => write!(f, "no such graph"),
            Self::NoSuchNode => write!(f, "no such node"),
            Self::NoSuchPort => write!(f, "no such port"),
            Self::Direction => write!(f, "wires go from an output to an input"),
            Self::TypeMismatch { from, to } => write!(f, "cannot wire {from:?} into {to:?}"),
            Self::Recursive => write!(f, "a block cannot contain itself"),
            Self::PinInSelection => write!(f, "pins cannot move into another block"),
            Self::PinAtRoot => write!(f, "pins only exist inside a block"),
            Self::EmptySelection => write!(f, "nothing selected"),
            Self::NotABlock => write!(f, "not a block instance"),
            Self::InUse(n) => write!(f, "block is used {n} time(s)"),
        }
    }
}

impl std::error::Error for EditError {}

impl Project {
    /// Adds a node to a graph.
    ///
    /// # Errors
    /// Unknown graph or block, a pin at the root, or a block instance that
    /// would nest a block inside itself.
    pub fn add_node(&mut self, scope: Scope, node: Node) -> Result<NodeId, EditError> {
        self.graph(scope).ok_or(EditError::NoSuchGraph)?;
        match (&node.kind, scope) {
            (Kind::PinIn { .. } | Kind::PinOut { .. }, Scope::Root) => return Err(EditError::PinAtRoot),
            (Kind::Block(b), _) if !self.blocks.contains_key(b) => return Err(EditError::NotABlock),
            (Kind::Block(b), Scope::Block(s)) if *b == s || self.reaches(*b, s) => {
                return Err(EditError::Recursive);
            }
            _ => {}
        }
        Ok(self.graph_mut(scope).expect("checked above").insert(node))
    }

    /// Wires an output to an input, replacing whatever fed that input.
    ///
    /// # Errors
    /// Unknown graph, node or port, or the two ports differ in type.
    pub fn connect(&mut self, scope: Scope, wire: Wire) -> Result<(), EditError> {
        let g = self.graph(scope).ok_or(EditError::NoSuchGraph)?;
        let (from, to) = (g.nodes.get(&wire.from), g.nodes.get(&wire.to));
        let (Some(from), Some(to)) = (from, to) else {
            return Err(EditError::NoSuchNode);
        };
        let out_ty = self
            .ports(&from.kind)
            .1
            .into_iter()
            .find(|p| p.key == wire.from_port)
            .map(|p| p.ty);
        let in_ty = self
            .ports(&to.kind)
            .0
            .into_iter()
            .find(|p| p.key == wire.to_port)
            .map(|p| p.ty);
        let (Some(out_ty), Some(in_ty)) = (out_ty, in_ty) else {
            return Err(EditError::NoSuchPort);
        };
        if out_ty != in_ty {
            return Err(EditError::TypeMismatch {
                from: out_ty,
                to: in_ty,
            });
        }
        let g = self.graph_mut(scope).expect("checked above");
        g.wires
            .retain(|w| !(w.to == wire.to && w.to_port == wire.to_port));
        g.wires.push(wire);
        Ok(())
    }

    /// Removes the wire feeding an input; returns it.
    pub fn disconnect(&mut self, scope: Scope, to: NodeId, to_port: u32) -> Option<Wire> {
        let g = self.graph_mut(scope)?;
        let i = g.wires.iter().position(|w| w.to == to && w.to_port == to_port)?;
        Some(g.wires.remove(i))
    }

    /// Deletes nodes and every wire touching them. Removing a pin also drops
    /// the wires on that port in every instance of the block.
    pub fn remove_nodes(&mut self, scope: Scope, ids: &BTreeSet<NodeId>) {
        if let Some(g) = self.graph_mut(scope) {
            g.nodes.retain(|id, _| !ids.contains(id));
            g.wires.retain(|w| !ids.contains(&w.from) && !ids.contains(&w.to));
        }
        self.prune();
    }

    /// Copies nodes (and the wires among them) `offset` away; returns the
    /// copies' ids in the order of `ids`.
    ///
    /// # Errors
    /// Unknown graph.
    pub fn duplicate(
        &mut self,
        scope: Scope,
        ids: &BTreeSet<NodeId>,
        offset: [f32; 2],
    ) -> Result<Vec<NodeId>, EditError> {
        let g = self.graph_mut(scope).ok_or(EditError::NoSuchGraph)?;
        let mut map = BTreeMap::new();
        for id in ids {
            let Some(n) = g.nodes.get(id) else { continue };
            let mut copy = n.clone();
            copy.pos = [n.pos[0] + offset[0], n.pos[1] + offset[1]];
            map.insert(*id, g.insert(copy));
        }
        let inner: Vec<Wire> = g
            .wires
            .iter()
            .filter_map(|w| {
                Some(Wire {
                    from: *map.get(&w.from)?,
                    to: *map.get(&w.to)?,
                    ..*w
                })
            })
            .collect();
        g.wires.extend(inner);
        Ok(ids.iter().filter_map(|id| map.get(id).copied()).collect())
    }

    /// Drops wires whose ports no longer exist or whose types no longer
    /// match, in every graph. Run after anything that changes a block's pins.
    pub fn prune(&mut self) {
        let scopes: Vec<Scope> = std::iter::once(Scope::Root)
            .chain(self.blocks.keys().map(|b| Scope::Block(*b)))
            .collect();
        for scope in scopes {
            let g = self.graph(scope).expect("listed above");
            let keep: Vec<bool> = g.wires.iter().map(|w| self.wire_ok(g, w)).collect();
            let g = self.graph_mut(scope).expect("listed above");
            let mut keep = keep.into_iter();
            g.wires.retain(|_| keep.next().unwrap_or(false));
            let mut seen = BTreeSet::new();
            g.wires.retain(|w| seen.insert((w.to, w.to_port)));
        }
    }

    fn wire_ok(&self, g: &Graph, w: &Wire) -> bool {
        let (Some(from), Some(to)) = (g.nodes.get(&w.from), g.nodes.get(&w.to)) else {
            return false;
        };
        let out = self
            .ports(&from.kind)
            .1
            .into_iter()
            .find(|p| p.key == w.from_port);
        let inp = self.ports(&to.kind).0.into_iter().find(|p| p.key == w.to_port);
        matches!((out, inp), (Some(o), Some(i)) if o.ty == i.ty)
    }

    /// Creates an empty block definition.
    pub fn new_block(&mut self, name: &str) -> BlockId {
        let floor = self.blocks.keys().next_back().map_or(0, |b| b.0 + 1);
        let id = BlockId(self.next_block.max(floor));
        self.next_block = id.0 + 1;
        self.blocks.insert(
            id,
            BlockDef {
                name: name.into(),
                graph: Graph::default(),
            },
        );
        id
    }

    /// Deletes a block definition nothing uses.
    ///
    /// # Errors
    /// Unknown block, or it still has instances.
    pub fn delete_block(&mut self, block: BlockId) -> Result<(), EditError> {
        if !self.blocks.contains_key(&block) {
            return Err(EditError::NotABlock);
        }
        match self.uses(block) {
            0 => {
                self.blocks.remove(&block);
                Ok(())
            }
            n => Err(EditError::InUse(n)),
        }
    }

    /// Points one instance at a fresh copy of its definition, so it can be
    /// edited without touching the other instances.
    ///
    /// # Errors
    /// Unknown graph or node, or the node is not a block instance.
    pub fn make_unique(&mut self, scope: Scope, instance: NodeId) -> Result<BlockId, EditError> {
        let g = self.graph(scope).ok_or(EditError::NoSuchGraph)?;
        let Some(Kind::Block(old)) = g.nodes.get(&instance).map(|n| &n.kind) else {
            return Err(EditError::NotABlock);
        };
        let old = *old;
        let def = self.blocks.get(&old).ok_or(EditError::NotABlock)?.clone();
        let id = self.new_block(&format!("{} copy", def.name));
        self.blocks.get_mut(&id).expect("just made").graph = def.graph;
        let g = self.graph_mut(scope).expect("checked above");
        g.nodes.get_mut(&instance).expect("checked above").kind = Kind::Block(id);
        Ok(id)
    }

    /// Moves the selected nodes into a new function block and puts one
    /// instance of it where they were. Every wire that crossed the selection's
    /// edge becomes a pin: one input pin per outside source, one output pin
    /// per inside source, so fan-out stays a single pin.
    ///
    /// # Errors
    /// Unknown graph, an empty selection, or a selection holding pins.
    #[allow(clippy::too_many_lines)]
    pub fn collapse(
        &mut self,
        scope: Scope,
        ids: &BTreeSet<NodeId>,
        name: &str,
    ) -> Result<(BlockId, NodeId), EditError> {
        let g = self.graph(scope).ok_or(EditError::NoSuchGraph)?;
        let sel: BTreeSet<NodeId> = ids
            .iter()
            .copied()
            .filter(|id| g.nodes.contains_key(id))
            .collect();
        if sel.is_empty() {
            return Err(EditError::EmptySelection);
        }
        if sel
            .iter()
            .any(|id| matches!(g.nodes[id].kind, Kind::PinIn { .. } | Kind::PinOut { .. }))
        {
            return Err(EditError::PinInSelection);
        }

        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for id in &sel {
            let [x, y] = g.nodes[id].pos;
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }

        let mut inner = Graph::default();
        for id in &sel {
            inner.nodes.insert(*id, g.nodes[id].clone());
        }
        inner.next_id = sel.iter().next_back().map_or(0, |id| id.0 + 1);

        let port_info = |node: NodeId, key: u32, out: bool| {
            let ports = self.ports(&g.nodes[&node].kind);
            let list = if out { ports.1 } else { ports.0 };
            list.into_iter().find(|p| p.key == key).map(|p| (p.name, p.ty))
        };

        let mut outer_wires = Vec::new();
        // Outside source -> the input pin standing in for it.
        let mut in_pins: BTreeMap<(NodeId, u32), NodeId> = BTreeMap::new();
        // Inside source -> the output pin exporting it.
        let mut out_pins: BTreeMap<(NodeId, u32), NodeId> = BTreeMap::new();
        let mut names = BTreeSet::new();
        let mut unique = |base: String| {
            let base = if base.is_empty() { "P".to_string() } else { base };
            let mut name = base.clone();
            let mut n = 2;
            while !names.insert(name.clone()) {
                name = format!("{base}{n}");
                n += 1;
            }
            name
        };

        let mut crossing_in: Vec<Wire> = Vec::new();
        let mut crossing_out: Vec<Wire> = Vec::new();
        for w in &g.wires {
            match (sel.contains(&w.from), sel.contains(&w.to)) {
                (true, true) => inner.wires.push(*w),
                (false, true) => crossing_in.push(*w),
                (true, false) => crossing_out.push(*w),
                (false, false) => outer_wires.push(*w),
            }
        }
        crossing_in.sort_by(|a, b| (g.nodes[&a.to].pos[1].total_cmp(&g.nodes[&b.to].pos[1])).then(a.cmp(b)));
        crossing_out
            .sort_by(|a, b| (g.nodes[&a.from].pos[1].total_cmp(&g.nodes[&b.from].pos[1])).then(a.cmp(b)));

        let instance = NodeId(g.next_id.max(g.nodes.keys().next_back().map_or(0, |id| id.0 + 1)));
        let mut pending_outer = Vec::new();
        let mut row = 0.0_f32;
        for w in &crossing_in {
            let pin = if let Some(p) = in_pins.get(&(w.from, w.from_port)) {
                *p
            } else {
                let Some((name, ty)) = port_info(w.to, w.to_port, false) else {
                    continue;
                };
                let pin = inner.insert(Node::new(
                    Kind::PinIn {
                        name: unique(name),
                        ty,
                    },
                    [x0 - 200.0, y0 + row * 60.0],
                ));
                row += 1.0;
                in_pins.insert((w.from, w.from_port), pin);
                pending_outer.push(Wire {
                    from: w.from,
                    from_port: w.from_port,
                    to: instance,
                    to_port: pin.0,
                });
                pin
            };
            inner.wires.push(Wire {
                from: pin,
                from_port: 0,
                to: w.to,
                to_port: w.to_port,
            });
        }
        row = 0.0;
        for w in &crossing_out {
            let pin = if let Some(p) = out_pins.get(&(w.from, w.from_port)) {
                *p
            } else {
                let Some((name, ty)) = port_info(w.from, w.from_port, true) else {
                    continue;
                };
                let pin = inner.insert(Node::new(
                    Kind::PinOut {
                        name: unique(name),
                        ty,
                    },
                    [x1 + 260.0, y0 + row * 60.0],
                ));
                row += 1.0;
                out_pins.insert((w.from, w.from_port), pin);
                inner.wires.push(Wire {
                    from: w.from,
                    from_port: w.from_port,
                    to: pin,
                    to_port: 0,
                });
                pin
            };
            pending_outer.push(Wire {
                from: instance,
                from_port: pin.0,
                to: w.to,
                to_port: w.to_port,
            });
        }

        let block = self.new_block(name);
        self.blocks.get_mut(&block).expect("just made").graph = inner;
        let g = self.graph_mut(scope).expect("checked above");
        g.nodes.retain(|id, _| !sel.contains(id));
        g.wires = outer_wires;
        let placed = g.insert(Node::new(
            Kind::Block(block),
            [f32::midpoint(x0, x1), f32::midpoint(y0, y1)],
        ));
        // `instance` was a placeholder above every id in use; swap in the
        // real one.
        g.wires.extend(pending_outer.into_iter().map(|mut w| {
            if w.from == instance {
                w.from = placed;
            }
            if w.to == instance {
                w.to = placed;
            }
            w
        }));
        Ok((block, placed))
    }

    /// Replaces a block instance with a copy of its contents, wired the way
    /// the instance was. The definition stays (other instances may use it).
    /// Returns the new nodes' ids.
    ///
    /// # Errors
    /// Unknown graph or node, or the node is not a block instance.
    pub fn explode(&mut self, scope: Scope, instance: NodeId) -> Result<Vec<NodeId>, EditError> {
        let g = self.graph(scope).ok_or(EditError::NoSuchGraph)?;
        let node = g.nodes.get(&instance).ok_or(EditError::NoSuchNode)?;
        let Kind::Block(b) = node.kind else {
            return Err(EditError::NotABlock);
        };
        let def = self.blocks.get(&b).ok_or(EditError::NotABlock)?.graph.clone();
        let at = node.pos;

        let (mut x0, mut y0) = (f32::MAX, f32::MAX);
        for n in def.nodes.values() {
            if !matches!(n.kind, Kind::PinIn { .. } | Kind::PinOut { .. }) {
                (x0, y0) = (x0.min(n.pos[0]), y0.min(n.pos[1]));
            }
        }
        let g = self.graph_mut(scope).expect("checked above");
        let mut map = BTreeMap::new();
        for (id, n) in &def.nodes {
            if matches!(n.kind, Kind::PinIn { .. } | Kind::PinOut { .. }) {
                continue;
            }
            let mut copy = n.clone();
            copy.pos = [at[0] + n.pos[0] - x0, at[1] + n.pos[1] - y0];
            map.insert(*id, g.insert(copy));
        }
        let outside_in: Vec<Wire> = g.wires.iter().filter(|w| w.to == instance).copied().collect();
        let outside_out: Vec<Wire> = g.wires.iter().filter(|w| w.from == instance).copied().collect();
        g.wires.retain(|w| w.from != instance && w.to != instance);
        g.nodes.remove(&instance);

        let mut wires = Vec::new();
        for w in &def.wires {
            match (map.get(&w.from), map.get(&w.to)) {
                (Some(&from), Some(&to)) => wires.push(Wire { from, to, ..*w }),
                // Input pin -> inside node: feed it from what fed the pin.
                (None, Some(&to)) => {
                    if let Some(src) = outside_in.iter().find(|o| o.to_port == w.from.0) {
                        wires.push(Wire {
                            from: src.from,
                            from_port: src.from_port,
                            to,
                            to_port: w.to_port,
                        });
                    }
                }
                // Inside node -> output pin: feed what the pin fed.
                (Some(&from), None) => {
                    for dst in outside_out.iter().filter(|o| o.from_port == w.to.0) {
                        wires.push(Wire {
                            from,
                            from_port: w.from_port,
                            to: dst.to,
                            to_port: dst.to_port,
                        });
                    }
                }
                // Pin straight through to pin.
                (None, None) => {
                    if let Some(src) = outside_in.iter().find(|o| o.to_port == w.from.0) {
                        for dst in outside_out.iter().filter(|o| o.from_port == w.to.0) {
                            wires.push(Wire {
                                from: src.from,
                                from_port: src.from_port,
                                to: dst.to,
                                to_port: dst.to_port,
                            });
                        }
                    }
                }
            }
        }
        g.wires.extend(wires);
        self.prune();
        Ok(map.into_values().collect())
    }
}
