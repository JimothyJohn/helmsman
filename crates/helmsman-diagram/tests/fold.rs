//! `helmsman_diagram::fold`: a set of blocks shown as one. The shapes by
//! hand and the refusals, then a property over generated programs: any
//! folds whatever give a diagram that is well formed, laid out, and
//! carries the values of the diagram it was made from.

// Short names (d, v, f, n) read best against the library's own.
#![allow(clippy::many_single_char_names)]

mod common;

use std::time::Duration;

use common::{SIZES, build, check, prog, shape};
use helmsman_diagram::fold::{Fold, FoldKind, Input, Origin, Skipped, View, view};
use helmsman_diagram::{Diagram, NodeId, NodeKind, annotate};
use helmsman_iec::{Image, Value, compile};
use proptest::prelude::*;

fn diagram(vars: &str, body: &str) -> Diagram {
    build(&prog(vars, body)).1.program().unwrap().diagram.clone()
}

fn node(d: &Diagram, text: &str) -> NodeId {
    d.nodes
        .iter()
        .find(|n| n.text == text && !matches!(n.kind, NodeKind::Write { .. }))
        .unwrap_or_else(|| panic!("no block {text}"))
        .id
}

#[test]
fn no_folds_is_the_diagram_itself() {
    let d = diagram(
        "a, b, y : BOOL; n : INT;",
        "IF a AND NOT b THEN n := n + 1; ELSE n := 0; END_IF;\ny := n > 3 OR a;",
    );
    let v = view(&d, &[]);
    assert_eq!(v.diagram, d);
    assert!(v.folds.is_empty() && v.skipped.is_empty());
    for n in &d.nodes {
        assert_eq!(v.shown(n.id), Some(n.id));
        assert_eq!(v.original(n.id), Some(n.id));
    }
}

#[test]
fn a_fold_within_a_statement_keeps_the_wires_that_cross_its_edge() {
    let d = diagram("lvl : REAL; go, y : BOOL;", "y := lvl > 80.0 AND NOT go;");
    assert_eq!(
        shape(&d),
        "\
#0 read LVL
#1 const 80.0
#2 op > <- #0 #1
#3 read GO
#4 op AND <- #2 !#3
#5 write Y := <- #4
"
    );
    let chip = Fold::over_nodes(
        "chip",
        FoldKind::Chip,
        "LVL > 80.0",
        [NodeId(0), NodeId(1), NodeId(2)],
    );
    let v = view(&d, &[chip]);
    assert_eq!(
        shape(&v.diagram),
        "\
#0 fold LVL > 80.0
#1 read GO
#2 op AND <- #0 !#1
#3 write Y := <- #2
"
    );
    check(&v.diagram);
    assert_eq!(v.origin[0], Origin::Fold(0));
    assert_eq!(v.outs[0], [(NodeId(2), 0)]);
    assert_eq!(v.port(NodeId(2), 0), Some((NodeId(0), 0)));
    assert_eq!(v.port(NodeId(0), 0), None, "the read is inside the fold");
    assert_eq!(v.original(NodeId(2)), Some(NodeId(4)));
    // Where the text is follows the blocks that are still themselves.
    assert_eq!(v.diagram.ops.get(&NodeId(2)), d.ops.get(&NodeId(4)));
    assert_eq!(v.diagram.pins.get(&(NodeId(2), 1)), d.pins.get(&(NodeId(4), 1)));
    assert!(!v.diagram.ops.contains_key(&NodeId(0)));
}

#[test]
fn a_fold_fed_from_outside_has_a_pin_per_source_and_keeps_its_bubbles() {
    let d = diagram("a, b, c, y : BOOL;", "y := (a OR NOT b) AND c;");
    let or = node(&d, "A OR NOT B");
    let and = node(&d, "(A OR NOT B) AND C");
    let mut f = Fold::over_nodes("f", FoldKind::Latch, "both", [or, and]);
    f.names.insert((NodeId(0), 0), "set".into());
    let v = view(&d, &[f]);
    assert_eq!(
        shape(&v.diagram),
        "\
#0 read A
#1 read B
#2 read C
#3 fold both <- #0 !#1 #2
#4 write Y := <- #3
"
    );
    assert_eq!(v.diagram.node(NodeId(3)).ins, ["set", "B", "C"]);
    assert_eq!(
        v.ins[0][1],
        Input::Wire {
            from: (NodeId(1), 0),
            negated: true
        }
    );
    check(&v.diagram);
}

#[test]
fn a_fold_of_whole_statements_reads_and_writes_by_name() {
    let d = diagram(
        "a, b, y, z : BOOL; n : INT;",
        "n := 1;\nIF a THEN y := b; n := n + 1; ELSE y := FALSE; END_IF;\nz := y;\nn := n + 2;",
    );
    let f = Fold::over_stmts(&d, "sec", FoldKind::Section, "Middle", &[d.top[1], d.top[2]]);
    let v = view(&d, &[f]);
    assert_eq!(
        shape(&v.diagram),
        "\
#0 const 1
#1 write N := <- #0
#2 fold Middle <- #1
#3 read N
#4 const 2
#5 op + <- #3 #4
#6 write N := <- #5
"
    );
    let fold = v.diagram.node(NodeId(2));
    assert_eq!(fold.ins, ["N", "A", "B", "Y"]);
    assert_eq!(fold.outs, ["Y", "N", "Z"]);
    assert!(v.diagram.regions.is_empty(), "the IF's arms are inside the fold");
    assert_eq!(v.diagram.top.len(), 3);
    check(&v.diagram);
}

#[test]
fn an_arm_folds_inside_its_region_and_the_gate_stays() {
    let d = diagram(
        "a, b, y : BOOL; n : INT;",
        "IF a AND b THEN y := TRUE; n := n + 1; ELSE y := FALSE; END_IF;",
    );
    let body = d.regions[0].body.clone();
    let f = Fold::over_stmts(&d, "arm", FoldKind::Arm, "IF A AND B", &body);
    let v = view(&d, &[f]);
    assert_eq!(v.diagram.regions.len(), 2);
    assert_eq!(v.diagram.regions[0].body.len(), 1);
    assert_eq!(v.diagram.regions[0].gate, d.regions[0].gate);
    let fold = v
        .diagram
        .nodes
        .iter()
        .find(|n| matches!(n.kind, NodeKind::Fold { .. }))
        .unwrap();
    assert_eq!(fold.region, Some(v.diagram.regions[0].id));
    assert_eq!(fold.outs, ["Y", "N"]);
    check(&v.diagram);
}

#[test]
fn a_fold_that_holds_a_gate_drives_the_region_from_its_pin() {
    let d = diagram("lvl : REAL; y : BOOL;", "IF lvl > 80.0 THEN y := TRUE; END_IF;");
    let gate = d.regions[0].gate.unwrap().0;
    let members: Vec<NodeId> = d.inputs(gate).map(|w| w.from).chain([gate]).collect();
    let v = view(
        &d,
        &[Fold::over_nodes("c", FoldKind::Chip, "LVL > 80.0", members)],
    );
    let (g, port) = v.diagram.regions[0].gate.unwrap();
    assert!(matches!(v.diagram.node(g).kind, NodeKind::Fold { .. }));
    assert_eq!(v.outs[0][port], (gate, 0));
    check(&v.diagram);
}

#[test]
fn a_fold_that_cannot_be_drawn_is_left_out_and_says_why() {
    let d = diagram("a, b, c, y, z : BOOL;", "y := (a OR b) AND (a OR c);\nz := y;");
    let or1 = node(&d, "A OR B");
    let and = node(&d, "(A OR B) AND (A OR C)");
    let skipped = |f: Fold| -> Vec<(usize, Skipped)> {
        let v = view(&d, &[f]);
        assert_eq!(v.diagram, d, "the diagram as it was");
        v.skipped
    };
    let f = |nodes: Vec<NodeId>| Fold::over_nodes("f", FoldKind::Chip, "f", nodes);
    assert_eq!(skipped(f(vec![])), [(0, Skipped::Empty)]);
    assert_eq!(skipped(f(vec![NodeId(999)])), [(0, Skipped::Empty)]);
    // A read and the AND it reaches only through the ORs between.
    assert_eq!(skipped(f(vec![NodeId(0), and])), [(0, Skipped::NotConvex)]);
    // Blocks of two statements.
    let z = d.nodes.last().unwrap().id;
    assert_eq!(skipped(f(vec![or1, z])), [(0, Skipped::Spread)]);
    // Statements that are not next to each other, or not of one block.
    let d2 = diagram("a, y, z : BOOL;", "y := a;\nIF a THEN z := a; END_IF;\nz := y;");
    let run =
        |stmts: &[usize]| view(&d2, &[Fold::over_stmts(&d2, "s", FoldKind::Section, "s", stmts)]).skipped;
    assert_eq!(run(&[d2.top[0], d2.top[2]]), [(0, Skipped::NotARun)]);
    assert_eq!(run(&[d2.top[0], d2.regions[0].body[0]]), [(0, Skipped::NotARun)]);
    assert_eq!(run(&[99]), [(0, Skipped::Empty)]);
    // The second of two folds sharing a block.
    let v = view(&d, &[f(vec![or1]), f(vec![or1, and])]);
    assert_eq!(v.skipped, [(1, Skipped::Overlaps)]);
    assert_eq!(v.folds.len(), 1);
}

#[test]
fn a_folded_block_carries_the_values_of_what_it_stands_for() {
    let src = prog(
        "a AT %IX0.0 : BOOL; b AT %IX0.1 : BOOL; y, z : BOOL;",
        "y := a AND NOT b;\nz := y OR b;",
    );
    let (mut p, m) = build(&src);
    let d = &m.program().unwrap().diagram;
    let v = view(
        d,
        &[Fold::over_stmts(d, "s", FoldKind::Section, "first", &[d.top[0]])],
    );
    let fold = v.shown(NodeId(0)).unwrap();
    for inputs in 0..4u8 {
        let mut img = Image::new(SIZES);
        img.i[0] = inputs;
        let (r, t) = p.scan_traced(&mut img, Duration::from_millis(10));
        r.unwrap();
        let live = v.live(&annotate(d, &t, ""));
        assert_eq!(live.values.len(), v.diagram.nodes.len());
        assert_eq!(live.values[fold.0 as usize], [t.get("Y")], "inputs {inputs:02b}");
        assert_eq!(t.get("Y"), Some(Value::Bool(inputs == 1)));
        assert!(live.ran[fold.0 as usize]);
    }
}

/// Folds of `d` picked by `seeds`: runs of statements of the top or of
/// an arm, and an operator with the blocks of its statement that feed it.
fn folds_of(d: &Diagram, seeds: &[(u8, u16, u16)]) -> Vec<Fold> {
    let mut out = Vec::new();
    for (k, &(shape, a, b)) in seeds.iter().enumerate() {
        let key = format!("f{k}");
        if shape % 3 < 2 {
            let mut blocks: Vec<&[usize]> = vec![&d.top];
            blocks.extend(d.regions.iter().map(|r| r.body.as_slice()));
            let block = blocks[a as usize % blocks.len()];
            if block.is_empty() {
                continue;
            }
            let at = b as usize % block.len();
            let len = 1 + (a as usize / 7) % (block.len() - at);
            out.push(Fold::over_stmts(
                d,
                key,
                FoldKind::Section,
                "run",
                &block[at..at + len],
            ));
        } else {
            let ops: Vec<&helmsman_diagram::Node> = d
                .nodes
                .iter()
                .filter(|n| matches!(n.kind, NodeKind::Op { .. } | NodeKind::Func { .. }))
                .collect();
            if ops.is_empty() {
                continue;
            }
            let top = ops[a as usize % ops.len()];
            let mut set = vec![top.id];
            let mut todo = vec![top.id];
            let deep = b % 2 == 0;
            while let Some(n) = todo.pop() {
                for w in d.inputs(n) {
                    let src = d.node(w.from);
                    if (src.stmt, src.region) == (top.stmt, top.region) && !set.contains(&src.id) {
                        set.push(src.id);
                        if deep {
                            todo.push(src.id);
                        }
                    }
                }
            }
            out.push(Fold::over_nodes(key, FoldKind::Chip, "part", set));
        }
    }
    out
}

fn well_formed(d: &Diagram, v: &View) -> Result<(), TestCaseError> {
    let f = &v.diagram;
    let folded: usize = v.folds.iter().map(|x| x.nodes.len()).sum();
    prop_assert_eq!(f.nodes.len(), d.nodes.len() - folded + v.folds.len());
    prop_assert_eq!(v.origin.len(), f.nodes.len());
    for (i, n) in f.nodes.iter().enumerate() {
        prop_assert_eq!(n.id.0 as usize, i);
        match v.origin[i] {
            Origin::Node(o) => {
                prop_assert_eq!(v.shown(o), Some(n.id));
                prop_assert_eq!(&n.kind, &d.node(o).kind);
                prop_assert_eq!((&n.ins, &n.outs), (&d.node(o).ins, &d.node(o).outs));
            }
            Origin::Fold(k) => {
                let is_fold = matches!(n.kind, NodeKind::Fold { fold, .. } if fold as usize == k);
                prop_assert!(is_fold);
                prop_assert_eq!(n.ins.len(), v.ins[k].len());
                prop_assert_eq!(n.outs.len(), v.outs[k].len());
                for m in &v.folds[k].nodes {
                    prop_assert_eq!(v.shown(*m), Some(n.id));
                }
            }
        }
    }
    let mut fed = std::collections::HashSet::new();
    for w in &f.wires {
        prop_assert!(w.out < f.node(w.from).outs.len(), "{:?}", w);
        prop_assert!(w.inp < f.node(w.to).ins.len(), "{:?}", w);
        prop_assert!(w.from < w.to, "producers come first: {:?}", w);
        prop_assert!(fed.insert((w.to, w.inp)), "one wire into a pin: {:?}", w);
    }
    // Every wire of the diagram is in the view, or inside one fold.
    for w in &d.wires {
        let (a, b) = (v.shown(w.from).unwrap(), v.shown(w.to).unwrap());
        if a == b && v.original(a).is_none() {
            continue;
        }
        let from = v.port(w.from, w.out);
        prop_assert!(from.is_some(), "{:?} lost its source", w);
        let (fa, fo) = from.unwrap();
        prop_assert!(
            f.wires
                .iter()
                .any(|x| (x.from, x.out, x.to, x.negated) == (fa, fo, b, w.negated)),
            "{:?} is not in the view",
            w
        );
    }
    // Statements: each once, in a block that exists; regions under
    // their parents, gated by a real port.
    let mut places = vec![0usize; f.stmts.len()];
    for &s in f.top.iter().chain(f.regions.iter().flat_map(|r| &r.body)) {
        prop_assert!(s < f.stmts.len());
        places[s] += 1;
    }
    prop_assert!(places.iter().all(|&c| c == 1), "{:?}", places);
    for (i, r) in f.regions.iter().enumerate() {
        prop_assert_eq!(r.id.0 as usize, i);
        prop_assert!(r.parent.is_none_or(|p| p < r.id));
        if let Some((g, port)) = r.gate {
            prop_assert!(port < f.node(g).outs.len());
        }
        prop_assert_eq!(d.region(v.region_origin[i]).gate.is_some(), r.gate.is_some());
    }
    for n in &f.nodes {
        prop_assert!(n.region.is_none_or(|r| (r.0 as usize) < f.regions.len()));
        prop_assert!(f.stmts.iter().any(|s| s.id == n.stmt));
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(150))]

    #[test]
    fn any_folds_give_a_well_formed_laid_out_view_with_the_same_values(
        body in proptest::collection::vec(common::programs::stmt(), 1..7),
        seeds in proptest::collection::vec((any::<u8>(), any::<u16>(), any::<u16>()), 0..4),
        inputs in any::<u8>(),
    ) {
        let src = common::programs::program(&body, false);
        let mut p = compile(&src, SIZES).map_err(|e| TestCaseError::fail(format!("{e:?}\n{src}")))?;
        let m = helmsman_diagram::lower(&src, &p.trace_map()).unwrap();
        let mut img = Image::new(SIZES);
        img.i[0] = inputs;
        let (_, t) = p.scan_traced(&mut img, Duration::from_millis(10));
        for pou in &m.pous {
            let d = &pou.diagram;
            let folds = folds_of(d, &seeds);
            let v = view(d, &folds);
            prop_assert_eq!(&v, &view(d, &folds), "deterministic");
            prop_assert_eq!(v.folds.len() + v.skipped.len(), folds.len());
            if v.folds.is_empty() {
                prop_assert_eq!(&v.diagram, d);
            }
            well_formed(d, &v).map_err(|e| TestCaseError::fail(format!("{e}\n{folds:?}\n{src}")))?;
            check(&v.diagram);
            // The values: every output the view still shows is the
            // diagram's own.
            let prefix = if pou.name == "PULSE" { "F." } else { "" };
            let live = annotate(d, &t, prefix);
            let seen = v.live(&live);
            prop_assert_eq!(seen.values.len(), v.diagram.nodes.len());
            prop_assert_eq!(seen.gates.len(), v.diagram.regions.len());
            for n in &d.nodes {
                for (out, value) in live.values[n.id.0 as usize].iter().enumerate() {
                    if let Some((to, k)) = v.port(n.id, out) {
                        prop_assert_eq!(seen.values[to.0 as usize][k], *value);
                    }
                }
            }
            for (k, r) in v.region_origin.iter().enumerate() {
                prop_assert_eq!(seen.gates[k], live.gates[r.0 as usize]);
                prop_assert_eq!(v.region_shown(*r), Some(v.diagram.regions[k].id));
            }
        }
    }
}

/// The property above is only worth its name if its folds apply: on a
/// plain program both shapes do.
#[test]
fn the_generated_folds_are_ones_that_apply() {
    let d = diagram(
        "a, b, y : BOOL; n : INT;",
        "y := n > 3 AND a;\nIF a THEN n := n + 1; y := b; END_IF;\nn := n * 2;",
    );
    let folds = folds_of(&d, &[(0, 0, 2), (2, 0, 0), (1, 1, 0)]);
    let v = view(&d, &folds);
    assert_eq!(v.skipped, []);
    assert!(v.folds.iter().any(|f| f.stmts.is_empty()));
    assert!(v.folds.iter().any(|f| !f.stmts.is_empty()));
}

#[test]
fn a_comparison_with_a_literal_is_one_block_fed_by_the_variable() {
    let d = diagram(
        "lvl : REAL; n : INT; hot, y : BOOL;",
        "hot := lvl > 80.0 AND 3 <= n;\nIF n = 2 THEN y := TRUE; END_IF;",
    );
    let folds = helmsman_diagram::fold::chips::chips(&d);
    assert_eq!(
        folds.iter().map(|f| f.title.as_str()).collect::<Vec<_>>(),
        ["> 80.0", "3 <=", "= 2"]
    );
    let v = view(&d, &folds);
    assert_eq!(v.skipped, []);
    assert_eq!(
        shape(&v.diagram),
        "\
#0 read LVL
#1 fold > 80.0 <- #0
#2 read N
#3 fold 3 <= <- #2
#4 op AND <- #1 #3
#5 write HOT := <- #4
#6 read N
#7 fold = 2 <- #6
#8 const TRUE [R0]
#9 write Y := [R0] <- #8
R0 IF N = 2 gate=#7 body=1
"
    );
    check(&v.diagram);
}

#[test]
fn only_a_literal_compared_once_folds() {
    // Two variables, two literals, a named constant, a bubble: as they are.
    let src = "PROGRAM t\nVAR CONSTANT lim : INT := 5; END_VAR\nVAR a, b : INT; x, y : BOOL; END_VAR\n\
               y := a > b;\ny := 1 < 2;\ny := a > lim;\ny := x = NOT y;\nEND_PROGRAM\n";
    let d = build(src).1.program().unwrap().diagram.clone();
    assert_eq!(helmsman_diagram::fold::chips::chips(&d), []);
}

#[test]
fn the_path_that_drives_an_output_runs_through_the_fold_it_is_in() {
    let src = prog(
        "a AT %IX0.0 : BOOL; n : INT; y, z, u : BOOL;",
        "y := a AND n > 3;\nz := y;\nu := TRUE;",
    );
    let (mut p, m) = build(&src);
    let d = &m.program().unwrap().diagram;
    let mut folds = helmsman_diagram::fold::auto(d);
    folds.push(Fold::over_stmts(d, "s", FoldKind::Section, "second", &[d.top[1]]));
    let v = view(d, &folds);
    assert_eq!(v.skipped, []);
    let (r, t) = p.scan_traced(&mut Image::new(SIZES), Duration::from_millis(10));
    r.unwrap();
    let on = helmsman_diagram::drivers(&m, &t, 0, "", "Z");
    let seen = v.drivers(&on);
    assert_eq!(seen.nodes.len(), v.diagram.nodes.len());
    assert_eq!(seen.wires.len(), v.diagram.wires.len());
    let lit: Vec<&str> = v
        .diagram
        .nodes
        .iter()
        .filter(|n| seen.node(n.id))
        .map(|n| n.label.as_str())
        .collect();
    assert_eq!(lit, ["A", "N", "> 3", "AND", "Y :=", "second"]);
    // The wire into the fold is on the path; the unrelated statement is not.
    let into = v
        .diagram
        .wires
        .iter()
        .position(|w| v.fold_at(w.to).is_some_and(|f| f.key == "s"))
        .unwrap();
    assert!(seen.wire(into));
    assert!(!lit.contains(&"U :="));
}

#[test]
fn a_fold_can_keep_the_variables_it_reads_as_blocks_wired_into_it() {
    let d = diagram(
        "c, a, b, y, z : BOOL;",
        "IF c THEN y := a AND c; ELSE y := b; END_IF;\nz := y;",
    );
    let f = Fold::over_stmts(&d, "s", FoldKind::Select, "pick", &[d.top[0]]).with_blocks();
    let v = view(&d, &[f]);
    assert_eq!(v.skipped, []);
    // One block per variable (C is read twice in the IF), each wired in;
    // the condition comes in as a wire too.
    assert_eq!(
        shape(&v.diagram),
        "\
#0 read C
#1 read A
#2 read B
#3 fold pick <- #0 #1 #2
#4 read Y
#5 write Z := <- #4
"
    );
    assert_eq!(v.diagram.node(NodeId(3)).ins, ["C", "A", "B"]);
    assert_eq!(v.diagram.node(NodeId(3)).outs, ["Y"]);
    check(&v.diagram);
    // The kept blocks are the diagram's own first reads; a second read of
    // the same variable is shown by the fold.
    let reads_of_c: Vec<NodeId> = d
        .nodes
        .iter()
        .filter(|n| matches!(&n.kind, NodeKind::Read { path } if path == "C"))
        .map(|n| n.id)
        .collect();
    assert_eq!(reads_of_c.len(), 2);
    assert_eq!(v.original(NodeId(0)), Some(reads_of_c[0]));
    assert_eq!(v.shown(reads_of_c[1]), Some(NodeId(3)));
    assert!(!v.folds[0].nodes.contains(&reads_of_c[0]));
    // Without it, the same fold names them on its pins.
    let plain = view(
        &d,
        &[Fold::over_stmts(&d, "s", FoldKind::Select, "pick", &[d.top[0]])],
    );
    assert_eq!(plain.diagram.nodes.len(), 3);
}

#[test]
fn a_gate_read_from_outside_a_fold_is_one_of_its_inputs() {
    // The inner IF is gated by a write made before the fold, through a
    // bubble; the arm fold must show it.
    let d = diagram(
        "a, b, g, y : BOOL;",
        "g := a;\nIF b THEN\n  IF NOT g THEN y := TRUE; END_IF;\n  y := y;\nEND_IF;",
    );
    let body = d.regions[0].body.clone();
    let v = view(&d, &[Fold::over_stmts(&d, "arm", FoldKind::Arm, "IF B", &body)]);
    assert_eq!(v.skipped, []);
    let fold = v
        .diagram
        .nodes
        .iter()
        .find(|n| matches!(n.kind, NodeKind::Fold { .. }))
        .unwrap();
    let into: Vec<_> = v.diagram.inputs(fold.id).collect();
    assert_eq!(into.len(), 1, "{}", shape(&v.diagram));
    assert!(into[0].negated);
    assert!(matches!(&v.diagram.node(into[0].from).kind, NodeKind::Write { path } if path == "G"));
    assert_eq!(fold.ins[0], "G");
    check(&v.diagram);
}

#[test]
fn the_folds_a_diagram_opens_with_all_apply_and_the_larger_shape_wins() {
    let d = diagram(
        "x, n : INT; start, stop, m, ok, y : BOOL;",
        "m := (start OR m) AND NOT stop;\nok := x >= 10 AND x <= 20;\ny := n > 3;\nIF ok THEN n := 1; ELSE n := 2; END_IF;",
    );
    let folds = helmsman_diagram::fold::auto(&d);
    let v = view(&d, &folds);
    // The range took its two comparisons; the chip rule offered them too.
    assert!(
        v.skipped.iter().all(|(_, why)| *why == Skipped::Overlaps),
        "{:?}",
        v.skipped
    );
    let mut kinds: Vec<FoldKind> = v.folds.iter().map(|f| f.kind).collect();
    kinds.sort();
    assert_eq!(
        kinds,
        [FoldKind::Chip, FoldKind::Range, FoldKind::Latch, FoldKind::Select]
    );
    check(&v.diagram);
    // Every variable still has a block.
    for name in ["START", "STOP", "X", "N", "OK"] {
        assert!(
            v.diagram
                .nodes
                .iter()
                .any(|n| matches!(&n.kind, NodeKind::Read { path } if path == name)
                    || matches!(&n.kind, NodeKind::Write { path } if path == name)),
            "{name}\n{}",
            shape(&v.diagram)
        );
    }
}
