//! `helmsman_diagram::fold::idioms`: the latch, edge, select, set/reset and
//! range rules. Each shape and every commuted form it is meant to catch,
//! then the near-misses that must be left exactly as they are, then the
//! folded block against what the interpreter computed.

// Short names (d, v, f, p, t) read best against the library's own.
#![allow(clippy::many_single_char_names)]

mod common;

use std::collections::HashSet;
use std::time::Duration;

use common::{SIZES, build, check, prog, shape};
use helmsman_diagram::fold::idioms::{edges, latches, ranges, selects, set_resets};
use helmsman_diagram::fold::{Fold, FoldKind, View, view};
use helmsman_diagram::{Diagram, Node, NodeKind, annotate};
use helmsman_iec::{Image, Value};

fn diagram(vars: &str, body: &str) -> Diagram {
    build(&prog(vars, body)).1.program().unwrap().diagram.clone()
}

/// `folds` applied to `d` on their own: none skipped, a well-formed layout.
fn applied(d: &Diagram, folds: &[Fold]) -> View {
    let v = view(d, folds);
    assert!(v.skipped.is_empty(), "skipped {:?}\n{}", v.skipped, shape(d));
    assert_eq!(v.folds.len(), folds.len());
    check(&v.diagram);
    v
}

fn fold_blocks(v: &View) -> Vec<&Node> {
    v.diagram
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Fold { .. }))
        .collect()
}

fn keys_are_unique(folds: &[Fold]) {
    let keys: HashSet<&str> = folds.iter().map(|f| f.key.as_str()).collect();
    assert_eq!(keys.len(), folds.len(), "{folds:?}");
}

/// Every statement that must find nothing.
fn none(rule: fn(&Diagram) -> Vec<Fold>, vars: &str, bodies: &[&str]) {
    for body in bodies {
        let d = diagram(vars, body);
        let found = rule(&d);
        assert!(found.is_empty(), "{body}\n{found:?}\n{}", shape(&d));
        // Nothing found, nothing changed.
        assert_eq!(view(&d, &found).diagram, d);
    }
}

// ---------------------------------------------------------------- latches

#[test]
fn a_seal_in_is_one_latch_with_set_and_reset_pins() {
    let d = diagram("s, r, m : BOOL;", "m := (s OR m) AND NOT r;");
    let f = latches(&d);
    assert_eq!(f.len(), 1);
    assert_eq!(
        (f[0].kind, f[0].title.as_str(), f[0].key.as_str()),
        (FoldKind::Latch, "M", "latch:M:0")
    );
    let v = applied(&d, &f);
    assert_eq!(
        shape(&v.diagram),
        "\
#0 read S
#1 read M
#2 read R
#3 fold M <- #0 #1 !#2
#4 write M := <- #3
"
    );
    assert_eq!(v.diagram.node(fold_blocks(&v)[0].id).ins, ["set", "M", "reset"]);
}

#[test]
fn every_commuted_seal_in_is_found_with_the_right_pins() {
    // (source, pin names: the wires of the OR in order, then the reset)
    for (body, pins) in [
        ("m := (s OR m) AND NOT r;", ["set", "M", "reset"]),
        ("m := (m OR s) AND NOT r;", ["M", "set", "reset"]),
        ("m := NOT r AND (s OR m);", ["set", "M", "reset"]),
        ("m := NOT r AND (m OR s);", ["M", "set", "reset"]),
    ] {
        let d = diagram("s, r, m : BOOL;", body);
        let f = latches(&d);
        assert_eq!(f.len(), 1, "{body}");
        let v = applied(&d, &f);
        let blocks = fold_blocks(&v);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].ins, pins, "{body}");
        assert_eq!(blocks[0].label, "M");
        assert_eq!(v.outs[0].len(), 1, "{body}: one output, into the write");
        assert_eq!(
            v.diagram.nodes.len(),
            5,
            "{body}: the three reads, the fold, the write"
        );
    }
}

#[test]
fn a_latch_of_a_struct_member_or_inside_an_arm_is_found() {
    let d = diagram(
        "go, s, r, m : BOOL;",
        "IF go THEN m := (s OR m) AND NOT r; END_IF;",
    );
    let f = latches(&d);
    assert_eq!(f.len(), 1);
    let v = applied(&d, &f);
    assert_eq!(fold_blocks(&v)[0].region, Some(v.diagram.regions[0].id));
}

#[test]
fn two_latches_of_one_variable_get_two_keys_and_both_apply() {
    let d = diagram(
        "s, r, m : BOOL;",
        "m := (s OR m) AND NOT r;\nm := (s OR m) AND NOT r;",
    );
    let f = latches(&d);
    assert_eq!(f.len(), 2, "the second feeds back from the first's write");
    keys_are_unique(&f);
    assert_eq!((f[0].key.as_str(), f[1].key.as_str()), ("latch:M:0", "latch:M:1"));
    assert_eq!(fold_blocks(&applied(&d, &f)).len(), 2);
}

#[test]
fn a_shape_one_wire_off_a_seal_in_is_not_one() {
    none(
        latches,
        "s, r, q, t, m : BOOL;",
        &[
            // The feedback is another variable.
            "m := (s OR q) AND NOT r;",
            // It writes another variable than the one it holds.
            "q := (s OR m) AND NOT r;",
            // Three inputs on the OR, or on the AND.
            "m := (s OR m OR t) AND NOT r;",
            "m := (s OR m) AND NOT r AND t;",
            "m := (s OR m) AND NOT r AND NOT t;",
            // The bubble on the wrong pin, or none, or on both.
            "m := (s OR m) AND r;",
            "m := NOT (s OR m) AND r;",
            "m := NOT (s OR m) AND NOT r;",
            // A bubble on the set, or on the feedback.
            "m := (NOT s OR m) AND NOT r;",
            "m := (s OR NOT m) AND NOT r;",
            // Other operators.
            "m := (s AND m) AND NOT r;",
            "m := (s OR m) OR NOT r;",
            "m := (s XOR m) AND NOT r;",
            "m := (s OR m) AND NOT r OR t;",
            // The same signal as set and reset, or the variable itself as one.
            "m := (s OR m) AND NOT s;",
            "m := (m OR m) AND NOT r;",
            "m := (s OR m) AND NOT m;",
            "m := (m OR s) AND NOT m;",
            // The feedback is a value computed, not the variable.
            "m := (s OR (m AND t)) AND NOT r;",
            // The latch is not what is stored.
            "q := ((s OR m) AND NOT r) OR t;",
            "m := ((s OR m) AND NOT r) OR t;",
            "m := NOT ((s OR m) AND NOT r);",
        ],
    );
}

// ------------------------------------------------------------------ edges

#[test]
fn a_rising_edge_with_its_previous_value_is_one_block() {
    let d = diagram("x, xp, p : BOOL;", "p := x AND NOT xp;\nxp := x;");
    let f = edges(&d);
    assert_eq!(f.len(), 1);
    assert_eq!(
        (f[0].kind, f[0].title.as_str(), f[0].key.as_str()),
        (FoldKind::Edge, "rising X", "rising:X:0")
    );
    let v = applied(&d, &f);
    assert_eq!(
        shape(&v.diagram),
        "\
#0 read X
#1 read XP
#2 fold rising X <- #0 !#1
#3 write P := <- #2
#4 read X
#5 write XP := <- #4
"
    );
}

#[test]
fn every_commuted_edge_names_the_signal_not_its_previous_value() {
    for (body, title) in [
        ("p := x AND NOT xp;\nxp := x;", "rising X"),
        ("p := NOT xp AND x;\nxp := x;", "rising X"),
        ("p := NOT x AND xp;\nxp := x;", "falling X"),
        ("p := xp AND NOT x;\nxp := x;", "falling X"),
        // Other work between the edge and the store of the previous value.
        ("p := x AND NOT xp;\nq := p;\nxp := x;", "rising X"),
        // Inside one arm, both statements of it.
        ("IF q THEN p := x AND NOT xp; xp := x; END_IF;", "rising X"),
    ] {
        let d = diagram("x, xp, p, q : BOOL;", body);
        let f = edges(&d);
        assert_eq!(f.len(), 1, "{body}");
        assert_eq!(f[0].title, title, "{body}");
        let v = applied(&d, &f);
        assert_eq!(fold_blocks(&v)[0].label, title);
    }
}

#[test]
fn an_edge_without_its_pairing_statement_stays_open() {
    none(
        edges,
        "x, y, xp, p, q : BOOL; n : INT;",
        &[
            // No `xp := x` at all.
            "p := x AND NOT xp;",
            "p := NOT x AND xp;",
            // The store comes before the edge: it compares x with itself.
            "xp := x;\np := x AND NOT xp;",
            // The store is of another signal, or negated, or computed.
            "p := x AND NOT xp;\nxp := y;",
            "p := x AND NOT xp;\nxp := NOT x;",
            "p := x AND NOT xp;\nxp := x AND y;",
            // The store is in another block than the edge.
            "p := x AND NOT xp;\nIF q THEN xp := x; END_IF;",
            "IF q THEN p := x AND NOT xp; END_IF;\nxp := x;",
            // The previous value is assigned twice, or x changes first.
            "p := x AND NOT xp;\nxp := x;\nxp := y;",
            "p := x AND NOT xp;\nx := y;\nxp := x;",
            // Same polarity on both inputs, or the same variable twice.
            "p := x AND xp;\nxp := x;",
            "p := NOT x AND NOT xp;\nxp := x;",
            "p := x AND NOT x;\nxp := x;",
            // A wider gate, or another operator.
            "p := x AND NOT xp AND y;\nxp := x;",
            "p := x OR NOT xp;\nxp := x;",
            "p := x AND NOT (xp AND y);\nxp := x;",
            // Arithmetic that only looks alike.
            "p := n > 1 AND NOT xp;\nxp := n > 1;",
        ],
    );
}

#[test]
fn a_later_copy_back_does_not_confuse_which_is_the_previous_value() {
    // `x := xp` after `xp := x` changes nothing the edge reads: still rising X.
    let d = diagram("x, xp, p : BOOL;", "p := x AND NOT xp;\nxp := x;\nx := xp;");
    let f = edges(&d);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].title, "rising X");
    applied(&d, &f);
    // With only the copy `x := xp` the roles are the other way round:
    // x is the previous value of xp, and xp is the signal that fell.
    let d = diagram("x, xp, p : BOOL;", "p := x AND NOT xp;\nx := xp;");
    let f = edges(&d);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].title, "falling XP");
    applied(&d, &f);
}

// ---------------------------------------------------------------- selects

#[test]
fn an_if_else_of_one_assignment_each_is_a_select() {
    let d = diagram(
        "c, a, b, y, z : BOOL;",
        "IF c THEN y := a; ELSE y := b; END_IF;\nz := y;",
    );
    let f = selects(&d);
    assert_eq!(f.len(), 1);
    assert_eq!(
        (
            f[0].kind,
            f[0].title.as_str(),
            f[0].key.as_str(),
            f[0].stmts.clone()
        ),
        (FoldKind::Select, "Y := A or B", "select:Y:0", vec![d.top[0]])
    );
    let v = applied(&d, &f);
    assert_eq!(
        shape(&v.diagram),
        "\
#0 fold Y := A or B
#1 read Y
#2 write Z := <- #1
"
    );
    assert_eq!(v.diagram.regions.len(), 0);
}

#[test]
fn a_select_of_values_and_of_a_nested_arm() {
    let d = diagram(
        "c, k, y : BOOL; n, m : INT;",
        "IF c THEN n := 5; ELSE n := m + 1; END_IF;\nIF k THEN IF c THEN y := TRUE; ELSE y := FALSE; END_IF; END_IF;",
    );
    let f = selects(&d);
    assert_eq!(f.len(), 2);
    keys_are_unique(&f);
    assert_eq!(f[0].title, "N := 5 or M + 1");
    assert_eq!(f[1].title, "Y := TRUE or FALSE");
    let v = applied(&d, &f);
    assert_eq!(fold_blocks(&v).len(), 2);
}

#[test]
fn a_select_with_a_long_title_says_only_that_it_selects() {
    let d = diagram(
        "c : BOOL; a, b, c1, c2, n : INT;",
        "IF c THEN n := a + b + c1 + c2 + a + b; ELSE n := c2 * c1 * b * a * c1; END_IF;",
    );
    let f = selects(&d);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].title, "N := one of two");
    applied(&d, &f);
}

#[test]
fn an_if_that_does_anything_but_pick_one_of_two_is_not_a_select() {
    none(
        selects,
        "c, c2, a, b, y, z : BOOL; n : INT; arr : ARRAY[0..3] OF BOOL;",
        &[
            // No ELSE, an ELSIF, or an ELSIF as well as the ELSE.
            "IF c THEN y := a; END_IF;",
            "IF c THEN y := a; ELSIF c2 THEN y := b; END_IF;",
            "IF c THEN y := a; ELSIF c2 THEN y := b; ELSE y := z; END_IF;",
            // A second statement in either arm.
            "IF c THEN y := a; z := a; ELSE y := b; END_IF;",
            "IF c THEN y := a; ELSE y := b; z := b; END_IF;",
            "IF c THEN y := a; ELSE END_IF;",
            // Different variables.
            "IF c THEN y := a; ELSE z := b; END_IF;",
            // An arm that is not an assignment.
            "IF c THEN y := a; ELSE IF c2 THEN y := b; END_IF; END_IF;",
            "IF c THEN IF c2 THEN y := a; END_IF; ELSE y := b; END_IF;",
            // An element of an array is not a plain variable.
            "IF c THEN arr[1] := a; ELSE arr[1] := b; END_IF;",
            "IF c THEN arr[n] := a; ELSE arr[n] := b; END_IF;",
            // Not an IF.
            "CASE n OF 1: y := a; ELSE y := b; END_CASE;",
            "WHILE c DO y := a; END_WHILE;",
        ],
    );
}

// ------------------------------------------------------------- set_resets

#[test]
fn an_if_that_only_sets_or_clears_is_a_set_reset() {
    let d = diagram(
        "c, c2, y, z, w : BOOL;",
        "IF c THEN y := TRUE; END_IF;\nIF c THEN z := FALSE; END_IF;\nIF c THEN w := TRUE; ELSIF c2 THEN w := FALSE; END_IF;",
    );
    let f = set_resets(&d);
    let titles: Vec<&str> = f.iter().map(|f| f.title.as_str()).collect();
    assert_eq!(titles, ["set Y", "reset Z", "set/reset W"]);
    assert!(f.iter().all(|f| f.kind == FoldKind::SetReset));
    assert_eq!(f[0].key, "setreset:Y:0");
    keys_are_unique(&f);
    let v = applied(&d, &f);
    assert_eq!(fold_blocks(&v).len(), 3);
    assert_eq!(v.diagram.regions.len(), 0);
}

#[test]
fn elsif_arms_of_one_value_are_one_set() {
    let d = diagram(
        "a, b, c, y : BOOL;",
        "IF a THEN y := TRUE; ELSIF b THEN y := TRUE; ELSIF c THEN y := TRUE; END_IF;",
    );
    let f = set_resets(&d);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].title, "set Y");
    applied(&d, &f);
}

#[test]
fn a_set_reset_in_an_arm_folds_inside_it() {
    let d = diagram(
        "go, c, y : BOOL;",
        "IF go THEN IF c THEN y := TRUE; END_IF; END_IF;",
    );
    let f = set_resets(&d);
    assert_eq!(f.len(), 1);
    let v = applied(&d, &f);
    assert_eq!(v.diagram.regions.len(), 1);
    assert_eq!(fold_blocks(&v)[0].region, Some(v.diagram.regions[0].id));
}

#[test]
fn an_if_with_anything_else_in_it_is_not_a_set_reset() {
    none(
        set_resets,
        "c, c2, a, y, z : BOOL; n : INT; arr : ARRAY[0..3] OF BOOL;",
        &[
            // An ELSE, even of literals.
            "IF c THEN y := TRUE; ELSE y := FALSE; END_IF;",
            "IF c THEN y := TRUE; ELSIF c2 THEN y := FALSE; ELSE y := TRUE; END_IF;",
            // Two variables.
            "IF c THEN y := TRUE; ELSIF c2 THEN z := FALSE; END_IF;",
            // A value that is not a literal, in any arm.
            "IF c THEN y := a; END_IF;",
            "IF c THEN y := TRUE; ELSIF c2 THEN y := a; END_IF;",
            "IF c THEN y := NOT a; END_IF;",
            "IF c THEN y := TRUE AND a; END_IF;",
            // Numbers are not a set or a clear.
            "IF c THEN n := 1; END_IF;",
            "IF c THEN n := 0; END_IF;",
            // A second statement in an arm.
            "IF c THEN y := TRUE; z := TRUE; END_IF;",
            "IF c THEN y := TRUE; ELSIF c2 THEN y := FALSE; z := TRUE; END_IF;",
            // An empty arm.
            "IF c THEN END_IF;",
            // An array element.
            "IF c THEN arr[2] := TRUE; END_IF;",
            // Not an IF.
            "CASE n OF 1: y := TRUE; END_CASE;",
        ],
    );
}

// ----------------------------------------------------------------- ranges

#[test]
fn a_variable_between_two_literals_is_one_range_block() {
    let d = diagram("x : INT; y : BOOL;", "y := 10 <= x AND x <= 20;");
    let f = ranges(&d);
    assert_eq!(f.len(), 1);
    assert_eq!(
        (f[0].kind, f[0].title.as_str(), f[0].key.as_str()),
        (FoldKind::Range, "10 .. 20", "range:X:0")
    );
    assert_eq!(f[0].nodes.len(), 5);
    let v = applied(&d, &f);
    assert_eq!(
        shape(&v.diagram),
        "\
#0 read X
#1 fold 10 .. 20 <- #0
#2 write Y := <- #1
"
    );
    assert_eq!(fold_blocks(&v)[0].ins, ["X"]);
}

#[test]
fn every_way_to_write_a_range_is_found_with_honest_bounds() {
    for (body, title) in [
        ("10 <= x AND x <= 20", "10 .. 20"),
        ("x >= 10 AND x <= 20", "10 .. 20"),
        ("x <= 20 AND x >= 10", "10 .. 20"),
        ("20 >= x AND 10 <= x", "10 .. 20"),
        ("x <= 20 AND 10 <= x", "10 .. 20"),
        ("x >= 10 AND 20 >= x", "10 .. 20"),
        ("10 < x AND x < 20", "10 < .. < 20"),
        ("x > 10 AND x < 20", "10 < .. < 20"),
        ("x < 20 AND x > 10", "10 < .. < 20"),
        ("20 > x AND 10 < x", "10 < .. < 20"),
        ("x > 10 AND x <= 20", "10 < .. <= 20"),
        ("x >= 10 AND x < 20", "10 <= .. < 20"),
        ("20 > x AND x >= 10", "10 <= .. < 20"),
    ] {
        let d = diagram("x : INT; y : BOOL;", &format!("y := {body};"));
        let f = ranges(&d);
        assert_eq!(f.len(), 1, "{body}");
        assert_eq!(f[0].title, title, "{body}");
        let v = applied(&d, &f);
        assert_eq!(v.diagram.nodes.len(), 3, "{body}: read, fold, write");
    }
}

#[test]
fn a_range_of_reals_and_inside_a_bigger_expression() {
    let d = diagram("r : REAL; ok, y : BOOL;", "y := (0.5 <= r AND r <= 2.5) AND ok;");
    let f = ranges(&d);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].title, "0.5 .. 2.5");
    let v = applied(&d, &f);
    // The range, the other operand, the outer AND, the write and r.
    assert_eq!(v.diagram.nodes.len(), 5, "{}", shape(&v.diagram));
}

#[test]
fn a_range_is_valid_alongside_nothing_but_itself() {
    // Two ranges of the same variable in one program: two keys, both apply.
    let d = diagram(
        "x : INT; a, b : BOOL;",
        "a := x >= 1 AND x <= 5;\nb := x >= 10 AND x <= 50;",
    );
    let f = ranges(&d);
    assert_eq!(f.len(), 2);
    keys_are_unique(&f);
    assert_eq!(fold_blocks(&applied(&d, &f)).len(), 2);
}

#[test]
fn near_ranges_are_not_ranges() {
    none(
        ranges,
        "x, z, lo, hi : INT; a, y : BOOL; r : REAL;",
        &[
            // Two variables.
            "y := x >= 1 AND z <= 5;",
            "y := 1 <= x AND z <= 5;",
            // Both bounds on the same side.
            "y := x > 1 AND x > 5;",
            "y := x < 1 AND x < 5;",
            "y := 1 < x AND 5 < x;",
            "y := 1 > x AND x < 5;",
            // A bound that is a variable, or neither side a literal.
            "y := x >= lo AND x <= 5;",
            "y := x >= 1 AND x <= hi;",
            "y := lo <= x AND x <= hi;",
            // Not an inequality.
            "y := x = 1 AND x <= 5;",
            "y := x <> 1 AND x <= 5;",
            "y := x >= 1 AND x = 5;",
            // A third input, or the AND is something else.
            "y := x >= 1 AND x <= 5 AND a;",
            "y := x >= 1 AND a AND x <= 5;",
            "y := x >= 1 OR x <= 5;",
            "y := x >= 1 XOR x <= 5;",
            "y := x >= 1 AND a;",
            // A bubble on either comparison's wire, or the AND's.
            "y := x >= 1 AND NOT (x <= 5);",
            "y := NOT (x >= 1) AND x <= 5;",
            // Comparisons of two computed values, which are no one variable.
            "y := x + 1 >= 1 AND x + 1 <= 5;",
            "y := ABS(x) >= 1 AND ABS(x) <= 5;",
            // Two literals compared, a literal on both sides of one.
            "y := 1 <= 5 AND x <= 5;",
            // Reals against integers of other variables.
            "y := r >= 1.0 AND x <= 5;",
        ],
    );
}

// ------------------------------------------------------- live and values

fn scan_with(p: &mut helmsman_iec::Program, byte: u8) -> helmsman_iec::trace::Trace {
    let mut img = Image::new(SIZES);
    img.i[0] = byte;
    let (r, t) = p.scan_traced(&mut img, Duration::from_millis(10));
    r.unwrap();
    t
}

#[test]
fn a_folded_latch_carries_the_value_the_program_stored() {
    // Every sequence of three scans over every (set, reset): the latch's
    // value is the variable the interpreter wrote, held between scans.
    let src = prog(
        "s AT %IX0.0 : BOOL; r AT %IX0.1 : BOOL; m : BOOL;",
        "m := (s OR m) AND NOT r;",
    );
    for seq in 0..64u8 {
        let (mut p, module) = build(&src);
        let d = &module.program().unwrap().diagram;
        let f = latches(d);
        assert_eq!(f.len(), 1);
        let v = applied(d, &f);
        let block = v.shown(f[0].nodes.iter().copied().next().unwrap()).unwrap();
        for step in 0..3 {
            let inputs = (seq >> (2 * step)) & 3;
            let t = scan_with(&mut p, inputs);
            let live = v.live(&annotate(d, &t, ""));
            let stored = t.get("M");
            assert!(matches!(stored, Some(Value::Bool(_))));
            assert_eq!(
                live.values[block.0 as usize],
                [stored],
                "sequence {seq:06b} step {step}"
            );
            assert!(live.ran[block.0 as usize]);
        }
    }
}

#[test]
fn a_folded_range_carries_the_value_the_program_stored() {
    let src = prog("x AT %IB0 : BYTE; y : BOOL;", "y := x > 10 AND x <= 20;");
    let (mut p, module) = build(&src);
    let d = &module.program().unwrap().diagram;
    let f = ranges(d);
    assert_eq!(f.len(), 1);
    let v = applied(d, &f);
    let block = v.shown(f[0].nodes.iter().copied().next().unwrap()).unwrap();
    let mut inside = 0;
    for x in 0..=255u8 {
        let t = scan_with(&mut p, x);
        let live = v.live(&annotate(d, &t, ""));
        assert_eq!(live.values[block.0 as usize], [t.get("Y")], "x = {x}");
        assert_eq!(t.get("Y"), Some(Value::Bool((11..=20).contains(&x))));
        inside += u32::from((11..=20).contains(&x));
    }
    assert_eq!(inside, 10);
}

#[test]
fn a_range_under_a_negation_keeps_the_bubble_outside_the_fold() {
    let d = diagram("x : INT; y : BOOL;", "y := NOT (x >= 1 AND x <= 5);");
    let f = ranges(&d);
    assert_eq!(f.len(), 1);
    let v = applied(&d, &f);
    assert_eq!(
        shape(&v.diagram),
        "\
#0 read X
#1 fold 1 .. 5 <- #0
#2 write Y := <- !#1
"
    );
}

#[test]
fn an_if_nested_in_an_arm_is_judged_on_its_own() {
    let d = diagram(
        "c, a, y : BOOL;",
        "IF c THEN y := TRUE; IF a THEN y := FALSE; END_IF; END_IF;",
    );
    let f = set_resets(&d);
    assert_eq!(f.len(), 1, "only the inner IF is one assignment per arm");
    assert_eq!(f[0].title, "reset Y");
    applied(&d, &f);
}
