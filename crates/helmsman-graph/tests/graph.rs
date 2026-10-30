//! Contract tests: what the simulator computes, what edits allow, and that
//! collapsing nodes into a function block (or exploding it) never changes
//! what the outputs see.

#![allow(clippy::many_single_char_names)]

use std::collections::BTreeSet;

use helmsman_graph::{
    BlockId, CmpOp, EditError, Kind, MathOp, Node, NodeId, Project, Scope, Sim, Ty, Value, Wire,
};
use proptest::prelude::*;

const ROOT: Scope = Scope::Root;

fn add(p: &mut Project, scope: Scope, kind: Kind) -> NodeId {
    p.add_node(scope, Node::new(kind, [0.0, 0.0])).unwrap()
}

fn wire(p: &mut Project, scope: Scope, from: NodeId, from_port: u32, to: NodeId, to_port: u32) {
    p.connect(
        scope,
        Wire {
            from,
            from_port,
            to,
            to_port,
        },
    )
    .unwrap();
}

fn set_toggle(p: &mut Project, scope: Scope, id: NodeId, on: bool) {
    p.graph_mut(scope).unwrap().nodes.get_mut(&id).unwrap().kind = Kind::Toggle { on };
}

/// Value on output `port` of root node `id` after the last scan.
fn out(sim: &Sim, id: NodeId, port: u32) -> Value {
    sim.root
        .outs
        .get(&(id, port))
        .copied()
        .unwrap_or(Value::Bool(false))
}

/// What every root lamp and display is being fed, in id order.
fn observe(p: &Project, sim: &Sim) -> Vec<(NodeId, Value)> {
    p.root
        .nodes
        .iter()
        .filter(|(_, n)| matches!(n.kind, Kind::Lamp | Kind::Display))
        .map(|(id, n)| {
            let ty = if n.kind == Kind::Lamp { Ty::Bool } else { Ty::Num };
            let v = p
                .root
                .wire_into(*id, 0)
                .map_or(Value::zero(ty), |w| sim.root.out(w.from, w.from_port, ty));
            (*id, v)
        })
        .collect()
}

#[test]
fn ton_delays_and_resets_when_input_drops() {
    let mut p = Project::default();
    let t = add(&mut p, ROOT, Kind::Toggle { on: true });
    let ton = add(&mut p, ROOT, Kind::Ton { preset_ms: 100.0 });
    wire(&mut p, ROOT, t, 0, ton, 0);
    let mut sim = Sim::default();
    for _ in 0..9 {
        sim.scan(&p, 10.0);
    }
    assert_eq!(out(&sim, ton, 0), Value::Bool(false));
    assert_eq!(out(&sim, ton, 1), Value::Num(90.0));
    sim.scan(&p, 10.0);
    assert_eq!(out(&sim, ton, 0), Value::Bool(true));
    sim.scan(&p, 10.0);
    assert_eq!(out(&sim, ton, 1), Value::Num(100.0), "ET clamps at PT");
    set_toggle(&mut p, ROOT, t, false);
    sim.scan(&p, 10.0);
    assert_eq!(
        (out(&sim, ton, 0), out(&sim, ton, 1)),
        (Value::Bool(false), Value::Num(0.0))
    );
}

#[test]
fn tof_holds_after_input_drops() {
    let mut p = Project::default();
    let t = add(&mut p, ROOT, Kind::Toggle { on: true });
    let tof = add(&mut p, ROOT, Kind::Tof { preset_ms: 50.0 });
    wire(&mut p, ROOT, t, 0, tof, 0);
    let mut sim = Sim::default();
    sim.scan(&p, 10.0);
    assert_eq!(out(&sim, tof, 0), Value::Bool(true));
    set_toggle(&mut p, ROOT, t, false);
    for _ in 0..4 {
        sim.scan(&p, 10.0);
        assert_eq!(out(&sim, tof, 0), Value::Bool(true));
    }
    sim.scan(&p, 10.0);
    assert_eq!(out(&sim, tof, 0), Value::Bool(false));
}

#[test]
fn tof_never_fires_without_input() {
    let mut p = Project::default();
    let tof = add(&mut p, ROOT, Kind::Tof { preset_ms: 50.0 });
    let mut sim = Sim::default();
    sim.scan(&p, 10.0);
    assert_eq!(out(&sim, tof, 0), Value::Bool(false));
}

#[test]
fn tp_pulses_once_per_rising_edge_and_ignores_retrigger() {
    let mut p = Project::default();
    let t = add(&mut p, ROOT, Kind::Toggle { on: true });
    let tp = add(&mut p, ROOT, Kind::Tp { preset_ms: 30.0 });
    wire(&mut p, ROOT, t, 0, tp, 0);
    let mut sim = Sim::default();
    let mut highs = 0;
    for i in 0..20 {
        // Wiggle the input mid-pulse: must not restart it.
        set_toggle(&mut p, ROOT, t, i != 1);
        sim.scan(&p, 10.0);
        highs += usize::from(out(&sim, tp, 0).as_bool());
    }
    assert_eq!(highs, 3, "30 ms pulse at 10 ms scans");
}

#[test]
fn tonr_keeps_time_until_reset() {
    let mut p = Project::default();
    let run = add(&mut p, ROOT, Kind::Toggle { on: true });
    let r = add(&mut p, ROOT, Kind::Toggle { on: false });
    let tonr = add(&mut p, ROOT, Kind::Tonr { preset_ms: 100.0 });
    wire(&mut p, ROOT, run, 0, tonr, 0);
    wire(&mut p, ROOT, r, 0, tonr, 1);
    let mut sim = Sim::default();
    for _ in 0..5 {
        sim.scan(&p, 10.0);
    }
    set_toggle(&mut p, ROOT, run, false);
    sim.scan(&p, 10.0);
    assert_eq!(out(&sim, tonr, 1), Value::Num(50.0), "retentive");
    set_toggle(&mut p, ROOT, run, true);
    for _ in 0..5 {
        sim.scan(&p, 10.0);
    }
    assert_eq!(out(&sim, tonr, 0), Value::Bool(true));
    set_toggle(&mut p, ROOT, r, true);
    sim.scan(&p, 10.0);
    assert_eq!(
        (out(&sim, tonr, 0), out(&sim, tonr, 1)),
        (Value::Bool(false), Value::Num(0.0))
    );
}

#[test]
fn ctu_counts_rising_edges_and_resets() {
    let mut p = Project::default();
    let cu = add(&mut p, ROOT, Kind::Toggle { on: false });
    let r = add(&mut p, ROOT, Kind::Toggle { on: false });
    let ctu = add(&mut p, ROOT, Kind::Ctu { preset: 3.0 });
    wire(&mut p, ROOT, cu, 0, ctu, 0);
    wire(&mut p, ROOT, r, 0, ctu, 1);
    let mut sim = Sim::default();
    for i in 0..6 {
        set_toggle(&mut p, ROOT, cu, i % 2 == 1);
        sim.scan(&p, 10.0);
        sim.scan(&p, 10.0); // held high: still one count
    }
    assert_eq!(
        (out(&sim, ctu, 0), out(&sim, ctu, 1)),
        (Value::Bool(true), Value::Num(3.0))
    );
    set_toggle(&mut p, ROOT, r, true);
    set_toggle(&mut p, ROOT, cu, false);
    sim.scan(&p, 10.0);
    assert_eq!(
        (out(&sim, ctu, 0), out(&sim, ctu, 1)),
        (Value::Bool(false), Value::Num(0.0))
    );
}

#[test]
fn sr_is_set_dominant_and_rs_reset_dominant() {
    let mut p = Project::default();
    let s = add(&mut p, ROOT, Kind::Toggle { on: true });
    let r = add(&mut p, ROOT, Kind::Toggle { on: true });
    let sr = add(&mut p, ROOT, Kind::Sr);
    let rs = add(&mut p, ROOT, Kind::Rs);
    for l in [sr, rs] {
        wire(&mut p, ROOT, s, 0, l, 0);
        wire(&mut p, ROOT, r, 0, l, 1);
    }
    let mut sim = Sim::default();
    sim.scan(&p, 10.0);
    assert_eq!(
        (out(&sim, sr, 0), out(&sim, rs, 0)),
        (Value::Bool(true), Value::Bool(false))
    );
    // Release both: SR stays latched.
    set_toggle(&mut p, ROOT, s, false);
    set_toggle(&mut p, ROOT, r, false);
    sim.scan(&p, 10.0);
    assert_eq!(out(&sim, sr, 0), Value::Bool(true));
}

#[test]
fn edges_fire_for_exactly_one_scan() {
    let mut p = Project::default();
    let t = add(&mut p, ROOT, Kind::Toggle { on: false });
    let re = add(&mut p, ROOT, Kind::RTrig);
    let fe = add(&mut p, ROOT, Kind::FTrig);
    wire(&mut p, ROOT, t, 0, re, 0);
    wire(&mut p, ROOT, t, 0, fe, 0);
    let mut sim = Sim::default();
    let mut seen = vec![];
    for on in [false, true, true, false, false] {
        set_toggle(&mut p, ROOT, t, on);
        sim.scan(&p, 10.0);
        seen.push((out(&sim, re, 0).as_bool(), out(&sim, fe, 0).as_bool()));
    }
    assert_eq!(
        seen,
        [
            (false, false),
            (true, false),
            (false, false),
            (false, true),
            (false, false)
        ]
    );
}

#[test]
fn clock_is_a_square_wave() {
    let mut p = Project::default();
    let c = add(&mut p, ROOT, Kind::Clock { period_ms: 40.0 });
    let mut sim = Sim::default();
    let wave: Vec<bool> = (0..8)
        .map(|_| {
            sim.scan(&p, 10.0);
            out(&sim, c, 0).as_bool()
        })
        .collect();
    assert_eq!(wave, [true, false, false, true, true, false, false, true]);
}

#[test]
fn math_never_produces_nan_or_infinity() {
    let mut p = Project::default();
    let a = add(&mut p, ROOT, Kind::Const { value: 1.0 });
    let z = add(&mut p, ROOT, Kind::Const { value: 0.0 });
    let big = add(&mut p, ROOT, Kind::Const { value: f64::MAX });
    let div = add(&mut p, ROOT, Kind::Math(MathOp::Div));
    let mul = add(&mut p, ROOT, Kind::Math(MathOp::Mul));
    wire(&mut p, ROOT, a, 0, div, 0);
    wire(&mut p, ROOT, z, 0, div, 1);
    wire(&mut p, ROOT, big, 0, mul, 0);
    wire(&mut p, ROOT, big, 0, mul, 1);
    let mut sim = Sim::default();
    sim.scan(&p, 10.0);
    assert_eq!(out(&sim, div, 0), Value::Num(0.0));
    assert_eq!(out(&sim, mul, 0), Value::Num(0.0));
}

#[test]
fn seal_in_circuit_latches_through_a_feedback_wire() {
    // Start/stop with a holding contact: Q = (START OR Q) AND NOT STOP.
    let mut p = Project::default();
    let start = add(&mut p, ROOT, Kind::Button);
    let stop = add(&mut p, ROOT, Kind::Button);
    let or = add(&mut p, ROOT, Kind::Or { inputs: 2 });
    let not = add(&mut p, ROOT, Kind::Not);
    let and = add(&mut p, ROOT, Kind::And { inputs: 2 });
    wire(&mut p, ROOT, start, 0, or, 0);
    wire(&mut p, ROOT, and, 0, or, 1);
    wire(&mut p, ROOT, stop, 0, not, 0);
    wire(&mut p, ROOT, or, 0, and, 0);
    wire(&mut p, ROOT, not, 0, and, 1);
    let mut sim = Sim::default();
    sim.scan(&p, 10.0);
    assert!(!out(&sim, and, 0).as_bool());
    sim.held.insert(vec![start]);
    sim.scan(&p, 10.0);
    sim.held.clear();
    for _ in 0..3 {
        sim.scan(&p, 10.0);
        assert!(out(&sim, and, 0).as_bool(), "holds after START is released");
    }
    sim.held.insert(vec![stop]);
    sim.scan(&p, 10.0);
    sim.held.clear();
    sim.scan(&p, 10.0);
    assert!(!out(&sim, and, 0).as_bool());
}

/// A block "DELAY" wrapping a TON between an input pin and an output pin.
fn delay_block(p: &mut Project, preset_ms: f64) -> (BlockId, NodeId, NodeId) {
    let b = p.new_block("DELAY");
    let s = Scope::Block(b);
    let pin_in = add(
        p,
        s,
        Kind::PinIn {
            name: "IN".into(),
            ty: Ty::Bool,
        },
    );
    let ton = add(p, s, Kind::Ton { preset_ms });
    let pin_out = add(
        p,
        s,
        Kind::PinOut {
            name: "Q".into(),
            ty: Ty::Bool,
        },
    );
    wire(p, s, pin_in, 0, ton, 0);
    wire(p, s, ton, 0, pin_out, 0);
    (b, pin_in, pin_out)
}

#[test]
fn block_instances_share_a_definition_but_not_state() {
    let mut p = Project::default();
    let (b, pin_in, pin_out) = delay_block(&mut p, 30.0);
    let on = add(&mut p, ROOT, Kind::Toggle { on: true });
    let late = add(&mut p, ROOT, Kind::Toggle { on: false });
    let i1 = add(&mut p, ROOT, Kind::Block(b));
    let i2 = add(&mut p, ROOT, Kind::Block(b));
    wire(&mut p, ROOT, on, 0, i1, pin_in.0);
    wire(&mut p, ROOT, late, 0, i2, pin_in.0);
    let mut sim = Sim::default();
    for _ in 0..3 {
        sim.scan(&p, 10.0);
    }
    set_toggle(&mut p, ROOT, late, true);
    sim.scan(&p, 10.0);
    assert_eq!(out(&sim, i1, pin_out.0), Value::Bool(true));
    assert_eq!(
        out(&sim, i2, pin_out.0),
        Value::Bool(false),
        "second instance has its own timer"
    );

    // Editing the definition changes both instances.
    let g = p.graph_mut(Scope::Block(b)).unwrap();
    let ton = g
        .nodes
        .iter()
        .find(|(_, n)| matches!(n.kind, Kind::Ton { .. }))
        .map(|(id, _)| *id)
        .unwrap();
    g.nodes.get_mut(&ton).unwrap().kind = Kind::Ton { preset_ms: 1000.0 };
    sim.reset();
    for _ in 0..10 {
        sim.scan(&p, 10.0);
    }
    assert_eq!(out(&sim, i1, pin_out.0), Value::Bool(false));
}

#[test]
fn blocks_nest_three_deep() {
    let mut p = Project::default();
    let (inner, inner_in, inner_out) = delay_block(&mut p, 20.0);
    // MIDDLE = DELAY then NOT; OUTER = MIDDLE then NOT. Net: delayed copy.
    let wrap = |p: &mut Project, name: &str, child: BlockId, cin: NodeId, cout: NodeId| {
        let b = p.new_block(name);
        let s = Scope::Block(b);
        let i = add(
            p,
            s,
            Kind::PinIn {
                name: "IN".into(),
                ty: Ty::Bool,
            },
        );
        let c = add(p, s, Kind::Block(child));
        let n = add(p, s, Kind::Not);
        let o = add(
            p,
            s,
            Kind::PinOut {
                name: "Q".into(),
                ty: Ty::Bool,
            },
        );
        wire(p, s, i, 0, c, cin.0);
        wire(p, s, c, cout.0, n, 0);
        wire(p, s, n, 0, o, 0);
        (b, i, o)
    };
    let (mid, mid_in, mid_out) = wrap(&mut p, "MIDDLE", inner, inner_in, inner_out);
    let (outer, outer_in, outer_out) = wrap(&mut p, "OUTER", mid, mid_in, mid_out);
    let t = add(&mut p, ROOT, Kind::Toggle { on: true });
    let inst = add(&mut p, ROOT, Kind::Block(outer));
    wire(&mut p, ROOT, t, 0, inst, outer_in.0);
    let mut sim = Sim::default();
    sim.scan(&p, 10.0);
    assert_eq!(out(&sim, inst, outer_out.0), Value::Bool(false));
    sim.scan(&p, 10.0);
    assert_eq!(out(&sim, inst, outer_out.0), Value::Bool(true));
    // The innermost timer's state is reachable by path.
    let mid_node = p.blocks[&outer]
        .graph
        .nodes
        .iter()
        .find(|(_, n)| n.kind == Kind::Block(mid))
        .map(|(id, _)| *id)
        .unwrap();
    assert!(sim.root.at(&[inst, mid_node]).is_some());
}

#[test]
fn a_block_cannot_contain_itself_at_any_depth() {
    let mut p = Project::default();
    let a = p.new_block("A");
    let b = p.new_block("B");
    assert_eq!(
        p.add_node(Scope::Block(a), Node::new(Kind::Block(a), [0.0; 2])),
        Err(EditError::Recursive)
    );
    add(&mut p, Scope::Block(a), Kind::Block(b));
    assert_eq!(
        p.add_node(Scope::Block(b), Node::new(Kind::Block(a), [0.0; 2])),
        Err(EditError::Recursive)
    );
    // Unrelated nesting is fine.
    let c = p.new_block("C");
    add(&mut p, Scope::Block(b), Kind::Block(c));
}

#[test]
fn hand_edited_self_nesting_is_refused_on_load() {
    let mut p = Project::default();
    let a = p.new_block("A");
    p.blocks
        .get_mut(&a)
        .unwrap()
        .graph
        .insert(Node::new(Kind::Block(a), [0.0; 2]));
    let err = Project::from_json(&p.to_json()).unwrap_err();
    assert!(err.contains("contains itself"), "{err}");
}

#[test]
fn wires_are_typed_directed_and_one_per_input() {
    let mut p = Project::default();
    let t = add(&mut p, ROOT, Kind::Toggle { on: false });
    let c = add(&mut p, ROOT, Kind::Const { value: 1.0 });
    let lamp = add(&mut p, ROOT, Kind::Lamp);
    let t2 = add(&mut p, ROOT, Kind::Toggle { on: true });
    assert_eq!(
        p.connect(
            ROOT,
            Wire {
                from: c,
                from_port: 0,
                to: lamp,
                to_port: 0
            }
        ),
        Err(EditError::TypeMismatch {
            from: Ty::Num,
            to: Ty::Bool
        })
    );
    assert_eq!(
        p.connect(
            ROOT,
            Wire {
                from: lamp,
                from_port: 0,
                to: t,
                to_port: 0
            }
        ),
        Err(EditError::NoSuchPort)
    );
    assert_eq!(
        p.connect(
            ROOT,
            Wire {
                from: t,
                from_port: 7,
                to: lamp,
                to_port: 0
            }
        ),
        Err(EditError::NoSuchPort)
    );
    wire(&mut p, ROOT, t, 0, lamp, 0);
    wire(&mut p, ROOT, t2, 0, lamp, 0);
    assert_eq!(
        p.root.wires,
        [Wire {
            from: t2,
            from_port: 0,
            to: lamp,
            to_port: 0
        }]
    );
}

#[test]
fn pins_only_inside_blocks() {
    let mut p = Project::default();
    assert_eq!(
        p.add_node(
            ROOT,
            Node::new(
                Kind::PinIn {
                    name: "X".into(),
                    ty: Ty::Bool
                },
                [0.0; 2]
            )
        ),
        Err(EditError::PinAtRoot)
    );
}

#[test]
fn removing_a_pin_drops_the_wires_on_that_port_in_every_instance() {
    let mut p = Project::default();
    let (b, pin_in, _) = delay_block(&mut p, 10.0);
    let t = add(&mut p, ROOT, Kind::Toggle { on: true });
    let i = add(&mut p, ROOT, Kind::Block(b));
    wire(&mut p, ROOT, t, 0, i, pin_in.0);
    p.remove_nodes(Scope::Block(b), &BTreeSet::from([pin_in]));
    assert_eq!(p.root.wires, []);
}

#[test]
fn retyping_a_pin_drops_mismatched_wires_on_prune() {
    let mut p = Project::default();
    let (b, pin_in, _) = delay_block(&mut p, 10.0);
    let t = add(&mut p, ROOT, Kind::Toggle { on: true });
    let i = add(&mut p, ROOT, Kind::Block(b));
    wire(&mut p, ROOT, t, 0, i, pin_in.0);
    p.graph_mut(Scope::Block(b))
        .unwrap()
        .nodes
        .get_mut(&pin_in)
        .unwrap()
        .kind = Kind::PinIn {
        name: "IN".into(),
        ty: Ty::Num,
    };
    p.prune();
    assert_eq!(p.root.wires, []);
    assert!(
        p.blocks[&b].graph.wires.iter().all(|w| w.from != pin_in),
        "Num pin into TON's Bool IN"
    );
}

#[test]
fn block_ports_follow_pin_height_and_keep_their_keys() {
    let mut p = Project::default();
    let b = p.new_block("B");
    let s = Scope::Block(b);
    let lo = p
        .add_node(
            s,
            Node::new(
                Kind::PinIn {
                    name: "LO".into(),
                    ty: Ty::Bool,
                },
                [0.0, 100.0],
            ),
        )
        .unwrap();
    let hi = p
        .add_node(
            s,
            Node::new(
                Kind::PinIn {
                    name: "HI".into(),
                    ty: Ty::Bool,
                },
                [0.0, 0.0],
            ),
        )
        .unwrap();
    let names: Vec<String> = p.ports(&Kind::Block(b)).0.into_iter().map(|x| x.name).collect();
    assert_eq!(names, ["HI", "LO"]);
    let keys: Vec<u32> = p.ports(&Kind::Block(b)).0.into_iter().map(|x| x.key).collect();
    assert_eq!(keys, [hi.0, lo.0]);
}

#[test]
fn collapse_dedupes_fan_in_and_fan_out_into_single_pins() {
    let mut p = Project::default();
    let t = add(&mut p, ROOT, Kind::Toggle { on: true });
    let a = add(&mut p, ROOT, Kind::And { inputs: 2 });
    let lamp1 = add(&mut p, ROOT, Kind::Lamp);
    let lamp2 = add(&mut p, ROOT, Kind::Lamp);
    wire(&mut p, ROOT, t, 0, a, 0);
    wire(&mut p, ROOT, t, 0, a, 1);
    wire(&mut p, ROOT, a, 0, lamp1, 0);
    wire(&mut p, ROOT, a, 0, lamp2, 0);
    let (b, inst) = p.collapse(ROOT, &BTreeSet::from([a]), "G").unwrap();
    let (ins, outs) = p.ports(&Kind::Block(b));
    assert_eq!((ins.len(), outs.len()), (1, 1));
    assert_eq!(p.root.wires.iter().filter(|w| w.from == inst).count(), 2);
    let mut sim = Sim::default();
    sim.scan(&p, 10.0);
    assert_eq!(
        observe(&p, &sim),
        [(lamp1, Value::Bool(true)), (lamp2, Value::Bool(true))]
    );
}

#[test]
fn collapse_refuses_pins_and_empty_selections() {
    let mut p = Project::default();
    let (b, pin_in, _) = delay_block(&mut p, 10.0);
    assert_eq!(
        p.collapse(Scope::Block(b), &BTreeSet::from([pin_in]), "X"),
        Err(EditError::PinInSelection)
    );
    assert_eq!(
        p.collapse(ROOT, &BTreeSet::new(), "X"),
        Err(EditError::EmptySelection)
    );
    assert_eq!(
        p.collapse(ROOT, &BTreeSet::from([NodeId(99)]), "X"),
        Err(EditError::EmptySelection)
    );
}

#[test]
fn make_unique_detaches_one_instance() {
    let mut p = Project::default();
    let (b, _, _) = delay_block(&mut p, 10.0);
    let i1 = add(&mut p, ROOT, Kind::Block(b));
    let _i2 = add(&mut p, ROOT, Kind::Block(b));
    let c = p.make_unique(ROOT, i1).unwrap();
    assert_ne!(c, b);
    assert_eq!((p.uses(b), p.uses(c)), (1, 1));
    assert_eq!(p.blocks[&c].graph, p.blocks[&b].graph);
}

#[test]
fn delete_block_refuses_while_in_use() {
    let mut p = Project::default();
    let (b, _, _) = delay_block(&mut p, 10.0);
    let i = add(&mut p, ROOT, Kind::Block(b));
    assert_eq!(p.delete_block(b), Err(EditError::InUse(1)));
    p.remove_nodes(ROOT, &BTreeSet::from([i]));
    p.delete_block(b).unwrap();
    assert!(p.blocks.is_empty());
}

#[test]
fn json_round_trips() {
    let mut p = Project::default();
    let (b, pin_in, _) = delay_block(&mut p, 10.0);
    let t = add(&mut p, ROOT, Kind::Toggle { on: true });
    let i = add(&mut p, ROOT, Kind::Block(b));
    wire(&mut p, ROOT, t, 0, i, pin_in.0);
    add(&mut p, ROOT, Kind::Compare(CmpOp::Le));
    add(
        &mut p,
        ROOT,
        Kind::Note {
            text: "hi \"there\"\n".into(),
        },
    );
    assert_eq!(Project::from_json(&p.to_json()).unwrap(), p);
}

#[test]
fn deleting_a_node_forgets_its_sim_state() {
    let mut p = Project::default();
    let ton = add(&mut p, ROOT, Kind::Ton { preset_ms: 10.0 });
    let mut sim = Sim::default();
    sim.scan(&p, 10.0);
    assert!(sim.root.mem.contains_key(&ton));
    p.remove_nodes(ROOT, &BTreeSet::from([ton]));
    sim.scan(&p, 10.0);
    assert!(sim.root.mem.is_empty() && sim.root.outs.is_empty());
}

/// Deterministic graph from a tape of random numbers: `n` nodes in index
/// order, every input optionally fed from an earlier node's output of the
/// same type (so the graph is acyclic and index order is topological).
fn build(tape: &[u32], n: usize) -> (Project, Vec<NodeId>) {
    let kinds = [
        Kind::Toggle { on: false },
        Kind::Toggle { on: true },
        Kind::Clock { period_ms: 30.0 },
        Kind::Clock { period_ms: 70.0 },
        Kind::Const { value: 2.5 },
        Kind::And { inputs: 3 },
        Kind::Or { inputs: 2 },
        Kind::Xor,
        Kind::Not,
        Kind::Sr,
        Kind::Rs,
        Kind::RTrig,
        Kind::FTrig,
        Kind::Ton { preset_ms: 40.0 },
        Kind::Tof { preset_ms: 40.0 },
        Kind::Tp { preset_ms: 40.0 },
        Kind::Tonr { preset_ms: 60.0 },
        Kind::Ctu { preset: 2.0 },
        Kind::Math(MathOp::Add),
        Kind::Math(MathOp::Div),
        Kind::Compare(CmpOp::Gt),
        Kind::Select,
        Kind::Lamp,
        Kind::Display,
    ];
    let mut t = tape.iter().copied().cycle();
    let mut next = || t.next().unwrap_or(0) as usize;
    let mut p = Project::default();
    let mut ids = Vec::new();
    for k in 0..n {
        let kind = kinds[next() % kinds.len()].clone();
        let y = f32::from(u16::try_from(k).unwrap()) * 50.0;
        let id = p.add_node(ROOT, Node::new(kind.clone(), [0.0, y])).unwrap();
        let ins = p.ports(&kind).0;
        for port in ins {
            if next() % 4 == 0 {
                continue;
            }
            let sources: Vec<(NodeId, u32)> = ids
                .iter()
                .flat_map(|src: &NodeId| {
                    let kind = &p.root.nodes[src].kind;
                    p.ports(kind)
                        .1
                        .into_iter()
                        .filter(|o| o.ty == port.ty)
                        .map(|o| (*src, o.key))
                        .collect::<Vec<_>>()
                })
                .collect();
            if !sources.is_empty() {
                let (from, from_port) = sources[next() % sources.len()];
                p.connect(
                    ROOT,
                    Wire {
                        from,
                        from_port,
                        to: id,
                        to_port: port.key,
                    },
                )
                .unwrap();
            }
        }
        ids.push(id);
    }
    (p, ids)
}

fn run(p: &Project, scans: usize) -> Vec<Vec<(NodeId, Value)>> {
    let mut sim = Sim::default();
    (0..scans)
        .map(|_| {
            sim.scan(p, 10.0);
            observe(p, &sim)
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Collapsing a contiguous run of nodes (convex in a DAG built in index
    /// order) into a block, and exploding that block again, leaves every
    /// lamp and display seeing the same values on every scan.
    #[test]
    fn collapse_and_explode_preserve_behaviour(
        tape in prop::collection::vec(any::<u32>(), 64..256),
        n in 3usize..16,
        a in 0usize..16,
        len in 1usize..16,
    ) {
        let (mut p, ids) = build(&tape, n);
        let before = run(&p, 25);
        let sel: BTreeSet<NodeId> = ids
            .iter()
            .skip(a % n)
            .take(len)
            .filter(|id| !matches!(p.root.nodes[id].kind, Kind::Lamp | Kind::Display))
            .copied()
            .collect();
        prop_assume!(!sel.is_empty());
        let (_, inst) = p.collapse(ROOT, &sel, "B").unwrap();
        prop_assert_eq!(&run(&p, 25), &before, "after collapse");
        p.explode(ROOT, inst).unwrap();
        prop_assert_eq!(&run(&p, 25), &before, "after explode");
    }

    /// Nesting collapses: collapse, then collapse the instance with its
    /// neighbours, twice over. Still the same behaviour.
    #[test]
    fn nested_collapse_preserves_behaviour(
        tape in prop::collection::vec(any::<u32>(), 64..256),
        n in 4usize..14,
    ) {
        let (mut p, ids) = build(&tape, n);
        let before = run(&p, 20);
        let sinks = |p: &Project, id: &NodeId| matches!(p.root.nodes.get(id).map(|n| &n.kind), Some(Kind::Lamp | Kind::Display));
        let first: BTreeSet<NodeId> = ids[..n / 2].iter().filter(|id| !sinks(&p, id)).copied().collect();
        prop_assume!(!first.is_empty());
        let (_, inst) = p.collapse(ROOT, &first, "INNER").unwrap();
        prop_assert_eq!(&run(&p, 20), &before);
        // The instance replaced a prefix, so instance + the next node is still convex.
        let mut second = BTreeSet::from([inst]);
        if let Some(next) = ids[n / 2..].iter().find(|id| !sinks(&p, id)) {
            second.insert(*next);
        }
        let (outer, _) = p.collapse(ROOT, &second, "OUTER").unwrap();
        prop_assert!(p.reaches(outer, p.blocks.keys().copied().find(|b| p.blocks[b].name == "INNER").unwrap()));
        prop_assert_eq!(&run(&p, 20), &before);
    }

    /// Arbitrary text never panics the loader.
    #[test]
    fn from_json_never_panics(s in ".{0,200}") {
        let _ = Project::from_json(&s);
    }

    /// Edits on random ids never panic and never leave a mistyped or dangling wire.
    #[test]
    fn random_edits_keep_wires_valid(
        tape in prop::collection::vec(any::<u32>(), 64..256),
        ops in prop::collection::vec((0u8..5, any::<u32>(), any::<u32>(), 0u32..4, 0u32..4), 0..40),
    ) {
        let (mut p, _) = build(&tape, 10);
        for (op, x, y, px, py) in ops {
            let ids: Vec<NodeId> = p.root.nodes.keys().copied().collect();
            if ids.is_empty() { break; }
            let pick = |v: u32| ids[v as usize % ids.len()];
            match op {
                0 => { let _ = p.connect(ROOT, Wire { from: pick(x), from_port: px, to: pick(y), to_port: py }); }
                1 => { p.remove_nodes(ROOT, &BTreeSet::from([pick(x)])); }
                2 => { let _ = p.duplicate(ROOT, &BTreeSet::from([pick(x), pick(y)]), [10.0, 10.0]); }
                3 => { let _ = p.collapse(ROOT, &BTreeSet::from([pick(x), pick(y)]), "C"); }
                _ => { let _ = p.explode(ROOT, pick(x)); }
            }
            let before = p.clone();
            p.prune();
            prop_assert_eq!(&p, &before, "an edit left a wire prune would drop");
        }
        let mut sim = Sim::default();
        sim.scan(&p, 10.0);
    }
}
