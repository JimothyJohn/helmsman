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

#[test]
fn refitting_a_fresh_layout_changes_nothing() {
    let a = diagram(BODY);
    let mut b = a.clone();
    helmsman_diagram::layout::refit(&mut b);
    for (x, y) in a.regions.iter().zip(&b.regions) {
        let close = |p: f32, q: f32| (p - q).abs() < 0.01;
        assert!(
            close(x.rect.x, y.rect.x)
                && close(x.rect.y, y.rect.y)
                && close(x.rect.w, y.rect.w)
                && close(x.rect.h, y.rect.h),
            "{}: {:?} -> {:?}",
            x.label,
            x.rect,
            y.rect
        );
    }
}

/// Whether node `i` sits in region `r` or one nested in it.
fn within(d: &Diagram, i: usize, r: helmsman_diagram::RegionId) -> bool {
    let mut at = d.nodes[i].region;
    while let Some(q) = at {
        if q == r {
            return true;
        }
        at = d.region(q).parent;
    }
    false
}

#[test]
fn a_moved_node_stays_inside_its_regions_and_its_ports_follow() {
    let mut d = diagram(BODY);
    // A node in the nested IF.
    let n = d
        .nodes
        .iter()
        .find(|n| n.region.is_some_and(|r| d.region(r).parent.is_some()))
        .map(|n| n.id)
        .expect("a nested node");
    let before = d.node(n).rect;
    let r = &mut d.nodes[n.0 as usize].rect;
    r.x += 400.0;
    r.y += 300.0;
    helmsman_diagram::layout::refit(&mut d);
    let after = d.node(n).rect;
    assert_eq!((after.x, after.y), (before.x + 400.0, before.y + 300.0));
    let mut at = d.node(n).region;
    while let Some(r) = at {
        let outer = d.region(r).rect;
        assert!(
            after.x >= outer.x
                && after.y >= outer.y
                && after.right() <= outer.right()
                && after.bottom() <= outer.bottom(),
            "{after:?} outside {}: {outer:?}",
            d.region(r).label
        );
        at = d.region(r).parent;
    }
    assert!(
        d.size.0 >= after.right() && d.size.1 >= after.bottom(),
        "{:?}",
        d.size
    );
    // Wires are drawn from the node's rect: its ports moved with it.
    let (x0, y0) = helmsman_diagram::layout::out_port_at(&before, true, 0);
    let (x1, y1) = helmsman_diagram::layout::out_port_at(&after, true, 0);
    assert!((x1 - x0 - 400.0).abs() < 0.01 && (y1 - y0 - 300.0).abs() < 0.01);
}

#[test]
fn a_region_carries_exactly_what_is_in_it_and_its_box_follows() {
    let mut d = diagram(BODY);
    let outer = d.regions.iter().find(|r| r.parent.is_none()).unwrap().id;
    let members = helmsman_diagram::layout::inside(&d, outer);
    let want: Vec<_> = (0..d.nodes.len())
        .filter(|&i| within(&d, i, outer))
        .map(|i| d.nodes[i].id)
        .collect();
    assert_eq!(members, want);
    assert!(
        members.iter().any(|n| d.node(*n).region != Some(outer)),
        "nested regions' nodes too"
    );
    let region = d.region(outer).rect;
    for n in &members {
        let r = &mut d.nodes[n.0 as usize].rect;
        r.x += 50.0;
        r.y += 500.0;
    }
    helmsman_diagram::layout::refit(&mut d);
    let moved = d.region(outer).rect;
    assert!((moved.x - region.x - 50.0).abs() < 0.01 && (moved.y - region.y - 500.0).abs() < 0.01);
}

/// Nick, 2026-11-01: an operator is one box with its label in the
/// middle, as tall as its larger side's ports need; a variable keeps
/// its header band over a body.
#[test]
fn an_operator_is_as_tall_as_its_ports_and_a_variable_keeps_its_header() {
    use helmsman_diagram::NodeKind;
    use helmsman_diagram::layout::{HEADER, PORT_ROW, in_port, out_port};
    let d = diagram("out := go AND stop AND lamp;");
    let and = d
        .nodes
        .iter()
        .find(|n| matches!(&n.kind, NodeKind::Op { op } if op == "AND"))
        .expect("an AND");
    assert!(!and.kind.headed());
    #[allow(clippy::cast_precision_loss)]
    let rows = and.ins.len().max(and.outs.len()) as f32;
    assert!(and.ins.len() >= 2, "{:?}", and.ins);
    assert!(
        (and.rect.h - (rows * PORT_ROW + 8.0)).abs() < 0.01,
        "no header band: {} for {rows} rows",
        and.rect.h
    );
    // Its ports sit in the rows, the first just under the top padding.
    let (_, y0) = in_port(and, 0);
    assert!((y0 - (and.rect.y + 4.0 + PORT_ROW / 2.0)).abs() < 0.01);
    let (_, yo) = out_port(and, 0);
    assert!(yo > and.rect.y && yo < and.rect.bottom());
    let go = d
        .nodes
        .iter()
        .find(|n| matches!(&n.kind, NodeKind::Read { path } if path == "GO"))
        .expect("the read of go");
    assert!(go.kind.headed());
    assert!(
        (go.rect.h - (HEADER + PORT_ROW + 4.0)).abs() < 0.01,
        "{}",
        go.rect.h
    );
    let (_, y) = out_port(go, 0);
    assert!(y > go.rect.y + HEADER, "under the header: {y} vs {}", go.rect.y);
}
