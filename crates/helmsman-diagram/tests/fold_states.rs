//! `fold::states`: a CASE on a state variable as one block of states, and
//! the layout hook that sizes a fold block by its rows. Every fold the
//! rule returns applies on its own, with nothing skipped.

// Short names (d, v, f, n) read best against the library's own.
#![allow(clippy::many_single_char_names)]

mod common;

use std::fmt::Write as _;
use std::time::Duration;

use common::{SIZES, build, check, prog, shape};
use helmsman_diagram::fold::states::states;
use helmsman_diagram::fold::{Fold, FoldKind, Skipped, View, auto, view};
use helmsman_diagram::layout::{self, Body, HEADER, PORT_ROW};
use helmsman_diagram::live::Gate;
use helmsman_diagram::{Diagram, NodeKind, annotate};
use helmsman_iec::{Image, compile};

const VARS: &str = "step : INT := 2; a, y, z : BOOL; n : INT;";

fn diagram(vars: &str, body: &str) -> (Diagram, String) {
    let src = prog(vars, body);
    (build(&src).1.program().unwrap().diagram.clone(), src)
}

fn applied(d: &Diagram, folds: &[Fold]) -> View {
    let v = view(d, folds);
    assert!(v.skipped.is_empty(), "skipped {:?} of {folds:?}", v.skipped);
    assert_eq!(v.folds.len(), folds.len());
    check(&v.diagram);
    v
}

fn labels(f: &Fold) -> Vec<&str> {
    f.rows.iter().map(|r| r.label.as_str()).collect()
}

const SEQ: &str = "CASE step OF\n 0: y := TRUE;\n 1, 2: z := TRUE; y := FALSE;\n 3..5: n := n + 1;\nELSE\n a := TRUE;\nEND_CASE;";

#[test]
fn a_case_on_one_variable_is_one_fold_with_a_row_per_arm() {
    let (d, _) = diagram(VARS, SEQ);
    let fs = states(&d);
    assert_eq!(fs.len(), 1);
    let f = &fs[0];
    assert_eq!(f.kind, FoldKind::State);
    assert_eq!(f.title, "STEP");
    assert_eq!(f.key, "state:STEP:0");
    assert_eq!(labels(f), ["0", "1, 2", "3..5", "ELSE"]);
    assert_eq!(
        f.rows.iter().map(|r| r.region).collect::<Vec<_>>(),
        d.stmts[0].arms,
        "a row per arm, in order"
    );
    // As the diagram opens with it (`auto`): the variables keep blocks.
    let fs: Vec<Fold> = fs.into_iter().map(Fold::with_blocks).collect();
    let v = applied(&d, &fs);
    // The variable keeps its block, wired into the fold; the arms are gone.
    assert!(v.diagram.regions.is_empty(), "{}", shape(&v.diagram));
    let fold = v
        .diagram
        .nodes
        .iter()
        .find(|n| matches!(n.kind, NodeKind::Fold { .. }))
        .unwrap();
    assert_eq!(fold.ins[0], "STEP", "the selector is the first pin");
    for w in ["Y", "Z", "N", "A"] {
        assert!(fold.outs.iter().any(|o| o == w), "{w} in {:?}", fold.outs);
    }
    assert!(
        v.diagram
            .nodes
            .iter()
            .any(|n| matches!(&n.kind, NodeKind::Read { path } if path == "STEP"))
    );
}

#[test]
fn it_opens_with_the_diagram() {
    let (d, _) = diagram(VARS, SEQ);
    assert!(auto(&d).iter().any(|f| f.kind == FoldKind::State));
    let v = view(&d, &auto(&d));
    assert!(v.skipped.iter().all(|(_, why)| *why == Skipped::Overlaps));
    check(&v.diagram);
}

#[test]
fn only_a_plain_variable_with_two_arms_is_a_state() {
    for body in [
        // An expression, an element, a call.
        "CASE step + 1 OF 0: y := TRUE; 1: z := TRUE; END_CASE;",
        "CASE ABS(step) OF 0: y := TRUE; 1: z := TRUE; END_CASE;",
        // The variable as an earlier statement of the scan left it.
        "step := 2;\nCASE step OF 0: y := TRUE; 1: z := TRUE; END_CASE;",
        // One arm: nothing to tell apart.
        "CASE step OF 0: y := TRUE; END_CASE;",
        // An IF is not a CASE.
        "IF a THEN y := TRUE; ELSE z := TRUE; END_IF;",
        "y := a;",
    ] {
        let (d, _) = diagram(VARS, body);
        assert_eq!(states(&d), [], "{body}");
        assert!(auto(&d).iter().all(|f| f.kind != FoldKind::State), "{body}");
    }
}

#[test]
fn an_else_alone_is_a_state_only_beside_another() {
    let (d, _) = diagram(VARS, "CASE step OF ELSE y := TRUE; END_CASE;");
    assert_eq!(states(&d), []);
    let (d, _) = diagram(VARS, "CASE step OF 1: y := TRUE; ELSE z := TRUE; END_CASE;");
    assert_eq!(labels(&states(&d)[0]), ["1", "ELSE"]);
}

#[test]
fn an_arm_with_no_statements_is_a_row_like_any_other() {
    let (d, _) = diagram(VARS, "CASE step OF 0: ; 1: y := TRUE; 2: ; END_CASE;");
    let fs = states(&d);
    assert_eq!(labels(&fs[0]), ["0", "1", "2"]);
    applied(&d, &fs);
}

#[test]
fn two_cases_on_one_variable_are_told_apart_by_key() {
    let (d, _) = diagram(
        VARS,
        "CASE step OF 0: y := TRUE; 1: z := TRUE; END_CASE;\nCASE step OF 2: a := TRUE; 3: n := 1; END_CASE;",
    );
    let fs = states(&d);
    assert_eq!(
        fs.iter().map(|f| f.key.as_str()).collect::<Vec<_>>(),
        ["state:STEP:0", "state:STEP:1"]
    );
    applied(&d, &fs);
}

#[test]
fn a_case_inside_a_state_is_offered_and_waits_for_the_outer_one_to_open() {
    let (d, _) = diagram(
        "step, sub : INT; y, z : BOOL;",
        "CASE step OF\n 0: CASE sub OF 0: y := TRUE; 1: z := TRUE; END_CASE;\n 1: y := FALSE;\nEND_CASE;",
    );
    let fs = states(&d);
    assert_eq!(fs.len(), 2);
    assert_eq!(fs[0].title, "STEP");
    assert_eq!(fs[1].title, "SUB");
    // Together the outer one holds the inner one's blocks.
    let v = view(&d, &fs);
    assert_eq!(v.skipped, [(1, Skipped::Overlaps)]);
    assert_eq!(v.folds.len(), 1);
    check(&v.diagram);
    // The inner one alone, as with the outer one opened: it applies, and
    // is drawn inside the outer arm.
    let v = applied(&d, &fs[1..]);
    assert_eq!(v.diagram.regions.len(), 2, "the outer CASE's own arms");
    let fold = v
        .diagram
        .nodes
        .iter()
        .find(|n| matches!(n.kind, NodeKind::Fold { .. }))
        .unwrap();
    assert!(fold.region.is_some());
}

fn many(k: usize) -> String {
    let mut s = String::from("CASE step OF\n");
    for i in 0..k {
        writeln!(s, " {i}: n := {i};").unwrap();
    }
    s.push_str("END_CASE;");
    s
}

#[test]
fn many_states_make_the_block_taller_and_never_much_wider() {
    let (d3, _) = diagram(VARS, &many(3));
    let (d30, _) = diagram(VARS, &many(30));
    let block = |d: &Diagram| {
        let v = applied(d, &states(d));
        let n = v
            .diagram
            .nodes
            .iter()
            .find(|n| matches!(n.kind, NodeKind::Fold { .. }))
            .unwrap()
            .clone();
        (n, v)
    };
    let (small, _) = block(&d3);
    let (big, v) = block(&d30);
    assert!(big.rect.h >= HEADER + 30.0 * PORT_ROW, "{:?}", big.rect);
    assert!(big.rect.h > small.rect.h);
    assert!(
        big.rect.w <= small.rect.w + 40.0,
        "{:?} vs {:?}",
        big.rect,
        small.rect
    );
    // Every row has room of its own, inside the block, none over another.
    let rows: Vec<_> = (0..30).map(|k| layout::body_row(&big, k)).collect();
    for (k, r) in rows.iter().enumerate() {
        assert!(r.w >= 3.0 * 7.0 && r.h > 8.0, "{r:?}");
        assert!(r.x >= big.rect.x && r.right() <= big.rect.right(), "{r:?}");
        assert!(
            r.y >= big.rect.y + HEADER && r.bottom() <= big.rect.bottom(),
            "{r:?}"
        );
        if let Some(next) = rows.get(k + 1) {
            assert!(r.bottom() <= next.y, "{r:?} {next:?}");
        }
    }
    check(&v.diagram);
}

#[test]
fn a_long_label_is_cut_not_a_wide_block() {
    let labels: Vec<String> = (0..40).map(|i| i.to_string()).collect();
    let (d, _) = diagram(
        VARS,
        &format!(
            "CASE step OF\n {}: y := TRUE;\n 99: z := TRUE;\nEND_CASE;",
            labels.join(", ")
        ),
    );
    let v = applied(&d, &states(&d));
    let n = v
        .diagram
        .nodes
        .iter()
        .find(|n| matches!(n.kind, NodeKind::Fold { .. }))
        .unwrap();
    assert!(n.rect.w < 400.0, "{:?}", n.rect);
}

/// The regions that were taken in the scan of `body` with `step` at `at`.
fn taken(body: &str, at: i64) -> (Vec<String>, Vec<Gate>) {
    let src = prog(VARS, body).replace("step : INT := 2", &format!("step : INT := {at}"));
    let mut p = compile(&src, SIZES).unwrap();
    let m = helmsman_diagram::lower(&src, &p.trace_map()).unwrap();
    let mut img = Image::new(SIZES);
    let (_, t) = p.scan_traced(&mut img, Duration::from_millis(10));
    let d = &m.program().unwrap().diagram;
    let f = &states(d)[0];
    let live = annotate(d, &t, "");
    let seen = view(d, std::slice::from_ref(f)).live(&live);
    let gates: Vec<Gate> = f.rows.iter().map(|r| live.gates[r.region.0 as usize]).collect();
    assert_eq!(
        seen.gates.len(),
        live.gates.len() - f.rows.len(),
        "the arms are inside the block"
    );
    let lit = f
        .rows
        .iter()
        .zip(&gates)
        .filter(|(_, g)| g.open())
        .map(|(r, _)| r.label.clone())
        .collect();
    (lit, gates)
}

#[test]
fn the_row_of_the_arm_that_ran_is_the_one_open_gate() {
    for (at, lit) in [(0, "0"), (1, "1, 2"), (2, "1, 2"), (4, "3..5"), (9, "ELSE")] {
        assert_eq!(taken(SEQ, at).0, [lit], "step at {at}");
    }
}

#[test]
fn a_value_no_arm_has_lights_no_row() {
    let (lit, gates) = taken("CASE step OF 0: y := TRUE; 1: z := TRUE; END_CASE;", 7);
    assert_eq!(lit, Vec::<String>::new());
    assert!(gates.iter().all(|g| *g == Gate::Closed), "{gates:?}");
}

#[test]
fn a_case_that_was_not_reached_lights_nothing() {
    let (lit, gates) = taken(
        "IF a THEN CASE step OF 0: y := TRUE; 1: z := TRUE; END_CASE; END_IF;",
        0,
    );
    assert_eq!(lit, Vec::<String>::new());
    assert!(gates.iter().all(|g| *g == Gate::NotReached), "{gates:?}");
}

#[test]
fn the_layout_sizes_any_fold_block_by_the_body_it_is_asked_for() {
    let (d, _) = diagram(VARS, "y := a;");
    let mut v = view(&d, &[Fold::over_stmts(&d, "s", FoldKind::Section, "run", &d.top)]);
    let own = v
        .diagram
        .nodes
        .iter()
        .find(|n| matches!(n.kind, NodeKind::Fold { .. }))
        .unwrap()
        .rect;
    layout::layout_with(&mut v.diagram, &|n| {
        if matches!(n.kind, NodeKind::Fold { .. }) {
            Body { rows: 10, chars: 30 }
        } else {
            Body::default()
        }
    });
    let big = v
        .diagram
        .nodes
        .iter()
        .find(|n| matches!(n.kind, NodeKind::Fold { .. }))
        .unwrap()
        .rect;
    assert!(
        big.h >= HEADER + 10.0 * PORT_ROW && big.h > own.h,
        "{big:?} {own:?}"
    );
    assert!(big.w >= 30.0 * 7.0, "{big:?}");
    check(&v.diagram);
}
