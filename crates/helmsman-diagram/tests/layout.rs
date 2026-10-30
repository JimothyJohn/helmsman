//! Layout: deterministic, left to right, nothing overlapping, regions
//! around their contents, and stable under edits below.

mod common;

use common::{build, check, prog};
use helmsman_diagram::{Diagram, Rect};

const BODY: &str = "t(IN := go AND NOT stop, PT := T#1s);
IF t.Q THEN
  n := n + 1;
  IF n > 3 THEN lamp := TRUE; ELSE lamp := FALSE; END_IF;
ELSIF stop THEN
  n := 0;
END_IF;
CASE n OF 1: x := 1; 2..4: x := 2; ELSE x := 3; END_CASE;
FOR i := 0 TO 2 DO x := x + i; END_FOR;
out := lamp OR x > 1;";

const VARS: &str = "go, stop, lamp, out : BOOL; t : TON; n, x, i : INT;";

fn diagram(body: &str) -> Diagram {
    build(&prog(VARS, body)).1.program().unwrap().diagram.clone()
}

#[test]
fn the_same_source_lays_out_the_same_and_keeps_the_invariants() {
    let a = diagram(BODY);
    let b = diagram(BODY);
    assert_eq!(a, b);
    check(&a);
}

#[test]
fn inputs_are_left_and_the_assignment_is_rightmost() {
    let d = diagram("out := (go OR lamp) AND NOT stop;");
    let write = d.nodes.last().unwrap();
    for n in &d.nodes[..d.nodes.len() - 1] {
        assert!(n.rect.right() <= write.rect.x, "{n:?}");
    }
    let reads: Vec<f32> = d
        .nodes
        .iter()
        .filter(|n| n.ins.is_empty())
        .map(|n| n.rect.x)
        .collect();
    assert!(
        reads.iter().all(|&x| x == 0.0),
        "reads all in the first column: {reads:?}"
    );
}

#[test]
fn editing_a_statement_moves_nothing_above_it() {
    let a = diagram(BODY);
    let edited = BODY.replace("x := x + i", "x := x + i * 2 - n");
    let b = diagram(&edited);
    // Statements before the FOR (the call, the IF and its arms, the CASE).
    let for_stmt = a
        .stmts
        .iter()
        .position(|s| {
            a.regions
                .iter()
                .any(|r| r.stmt == s.id && r.label.starts_with("FOR"))
        })
        .unwrap();
    let before = |d: &Diagram| -> Vec<Rect> {
        d.nodes
            .iter()
            .filter(|n| d.stmts.iter().position(|s| s.id == n.stmt).unwrap() < for_stmt)
            .map(|n| n.rect)
            .collect()
    };
    assert_eq!(before(&a), before(&b));
    let regions = |d: &Diagram| -> Vec<Rect> {
        d.regions
            .iter()
            .filter(|r| !r.label.starts_with("FOR"))
            .map(|r| r.rect)
            .collect()
    };
    assert_eq!(regions(&a), regions(&b));
    assert_ne!(a.size, b.size, "the edit itself did change the layout");
}

#[test]
fn an_appended_statement_moves_nothing_already_there() {
    let a = diagram(BODY);
    let b = diagram(&format!("{BODY}\nlamp := NOT lamp;"));
    for (x, y) in a.nodes.iter().zip(&b.nodes) {
        assert_eq!(x.rect, y.rect, "{x:?}");
    }
    check(&b);
}

#[test]
fn hit_testing_finds_nodes_and_regions() {
    let d = diagram(BODY);
    for n in &d.nodes {
        let (x, y) = (n.rect.x + n.rect.w / 2.0, n.rect.y + n.rect.h / 2.0);
        assert_eq!(d.node_at(x, y), Some(n.id));
    }
    let inner = d.regions.iter().find(|r| r.parent.is_some()).unwrap();
    assert_eq!(
        d.region_at(inner.rect.x + 1.0, inner.rect.y + 1.0),
        Some(inner.id)
    );
    assert_eq!(d.node_at(-5.0, -5.0), None);
}
