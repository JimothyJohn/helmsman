//! `fold::table`: an OR of ANDs as one block of rows. Every fold the
//! rule returns applies on its own, with nothing skipped.

// Short names (d, v, f, n) read best against the library's own.
#![allow(clippy::many_single_char_names, clippy::cast_precision_loss)]

mod common;

use std::fmt::Write as _;
use std::time::Duration;

use common::{SIZES, build, check, prog};
use helmsman_diagram::fold::table::tables;
use helmsman_diagram::fold::{CELL_CHARS, Fold, FoldKind, Skipped, TABLE_CHARS, View, auto, view};
use helmsman_diagram::layout::{self, HEADER, PORT_ROW};
use helmsman_diagram::{Diagram, NodeKind, annotate};
use helmsman_iec::{Image, Value};

const VARS: &str = "a, b, c, d, e, f, y : BOOL; n, m : INT; w, v : WORD; t : TON;";

fn diagram(vars: &str, body: &str) -> Diagram {
    let src = prog(vars, body);
    build(&src).1.program().unwrap().diagram.clone()
}

fn applied(d: &Diagram, folds: &[Fold]) -> View {
    let v = view(d, folds);
    assert!(v.skipped.is_empty(), "skipped {:?} of {folds:?}", v.skipped);
    assert_eq!(v.folds.len(), folds.len());
    check(&v.diagram);
    v
}

/// The rows of a table as text: `A, !B` is a row needing A on and B off.
fn rows(f: &Fold) -> Vec<String> {
    f.table
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .map(|r| {
            r.iter()
                .map(|c| format!("{}{}", if c.negated { "!" } else { "" }, c.label))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .collect()
}

fn the_table(body: &str) -> Fold {
    let d = diagram(VARS, body);
    let fs = tables(&d);
    assert_eq!(fs.len(), 1, "{body}");
    fs.into_iter().next().unwrap()
}

fn fold_block(v: &View) -> &helmsman_diagram::Node {
    v.diagram
        .nodes
        .iter()
        .find(|n| matches!(n.kind, NodeKind::Fold { .. }))
        .unwrap()
}

#[test]
fn an_or_of_ands_is_one_fold_with_a_row_per_term() {
    let body = "y := (a AND b AND NOT c) OR (d AND e) OR f;";
    let d = diagram(VARS, body);
    let fs = tables(&d);
    assert_eq!(fs.len(), 1);
    let f = &fs[0];
    assert_eq!(f.kind, FoldKind::Table);
    assert_eq!(f.title, "Y");
    assert_eq!(f.key, "table:Y:0");
    assert_eq!(rows(f), ["A, B, !C", "D, E", "F"]);
    let v = applied(&d, &fs);
    // One block for the OR and its two ANDs; its pins are the six
    // signals, its one output the OR's.
    let n = fold_block(&v);
    assert_eq!(n.ins, ["A", "B", "C", "D", "E", "F"]);
    assert_eq!(f.nodes.len(), 3);
    assert_eq!(
        v.diagram
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::Fold { .. }))
            .count(),
        1
    );
}

#[test]
fn two_terms_are_a_table_and_so_are_five() {
    assert_eq!(rows(&the_table("y := a AND b OR c AND d;")), ["A, B", "C, D"]);
    assert_eq!(
        rows(&the_table(
            "y := a AND b OR c AND d OR e AND f OR a AND f OR NOT b AND NOT e;"
        )),
        ["A, B", "C, D", "E, F", "A, F", "!B, !E"]
    );
    // One AND beside a lone signal is enough.
    assert_eq!(rows(&the_table("y := a OR b AND c;")), ["A", "B, C"]);
}

#[test]
fn a_signal_in_several_rows_is_one_pin_whichever_way_each_row_needs_it() {
    let d = diagram(VARS, "y := a AND b OR NOT a AND c OR a AND NOT c;");
    let fs = tables(&d);
    assert_eq!(rows(&fs[0]), ["A, B", "!A, C", "A, !C"]);
    let v = applied(&d, &fs);
    let n = fold_block(&v);
    assert_eq!(n.ins, ["A", "B", "C"], "one pin per signal");
    // The cells carry the bubbles, not the pins.
    assert!(v.diagram.wires.iter().all(|w| !w.negated || w.to != n.id));
    // Every cell names a pin that is its signal.
    for row in &fs[0].table.as_ref().unwrap().rows {
        for c in row {
            let pin = v.ins[0].iter().position(
                |i| matches!(i, helmsman_diagram::fold::Input::Wire { from, .. } if *from == c.from),
            );
            assert_eq!(n.ins[pin.unwrap()], c.label);
        }
    }
}

#[test]
fn a_comparison_with_a_literal_is_a_cell_named_by_its_text() {
    let f = the_table("y := n > 80 AND a OR NOT (m <= 3) AND b OR 5 < n AND c;");
    assert_eq!(rows(&f), ["N > 80, A", "!M <= 3, B", "5 < N, C"]);
    // As the diagram opens, the comparisons keep their chips and feed the
    // table's pins.
    let d = diagram(VARS, "y := n > 80 AND a OR NOT (m <= 3) AND b;");
    let v = view(&d, &auto(&d));
    assert!(v.skipped.is_empty(), "{:?}", v.skipped);
    let mut kinds: Vec<FoldKind> = v.folds.iter().map(|f| f.kind).collect();
    kinds.sort();
    assert_eq!(kinds, [FoldKind::Chip, FoldKind::Chip, FoldKind::Table]);
    check(&v.diagram);
}

#[test]
fn a_function_block_output_and_an_earlier_write_are_signals_too() {
    let f = the_table("t(IN := a, PT := T#1s);\nb := t.Q;\ny := t.Q AND NOT c OR b AND d;");
    assert_eq!(rows(&f), ["T.Q, !C", "B, D"]);
}

#[test]
fn it_opens_with_the_diagram() {
    let d = diagram(VARS, "y := a AND b OR c AND d;");
    assert!(auto(&d).iter().any(|f| f.kind == FoldKind::Table));
    let v = view(&d, &auto(&d));
    assert!(v.skipped.iter().all(|(_, why)| *why == Skipped::Overlaps));
    assert!(v.folds.iter().any(|f| f.kind == FoldKind::Table));
    check(&v.diagram);
}

#[test]
fn an_if_condition_is_a_table_and_its_gate_reads_the_fold() {
    let d = diagram(VARS, "IF a AND b OR c AND NOT d THEN y := TRUE; END_IF;");
    let fs = tables(&d);
    assert_eq!(fs[0].title, "permissive");
    let v = applied(&d, &fs);
    let g = v.diagram.regions[0].gate.unwrap();
    assert!(matches!(v.diagram.node(g.0).kind, NodeKind::Fold { .. }));
}

#[test]
fn what_is_not_a_table_is_left_alone() {
    for body in [
        // A nested OR in a term, and an OR in an AND.
        "y := a AND (b OR c) OR d AND e;",
        "y := (a AND b OR c) OR d;",
        "y := (a AND b OR c AND d) AND e;",
        // A call, arithmetic, a comparison of two variables.
        "y := a AND ABS(n) > 3 OR b AND c;",
        "y := a AND n + 1 > 3 OR b AND c;",
        "y := a AND n > m OR b AND c;",
        // One term only, or a plain OR of signals.
        "y := a AND b AND c;",
        "y := a OR b OR c;",
        "y := a OR NOT b;",
        // A negated AND is a NAND, not a row.
        "y := NOT (a AND b) OR c AND d;",
        // A literal is no signal.
        "y := a AND TRUE OR b AND c;",
        // Bitwise on words.
        "w := v AND 16#F0 OR w AND 16#0F;",
        "w := w AND v OR v AND w;",
        // An AND of ORs is not what the card asks.
        "y := (a OR b) AND (c OR d);",
        // An XOR.
        "y := a XOR b OR c AND d;",
    ] {
        let d = diagram(VARS, body);
        assert_eq!(tables(&d), [], "{body}");
        assert!(auto(&d).iter().all(|f| f.kind != FoldKind::Table), "{body}");
    }
}

#[test]
fn an_and_read_by_something_else_too_is_not_a_row() {
    let d = diagram(VARS, "y := a AND b OR c AND d;\nf := a AND b;");
    // The two ANDs of different statements are different blocks: still one.
    assert_eq!(tables(&d).len(), 1);
    let d = diagram(VARS, "IF (a AND b) THEN y := TRUE; END_IF;");
    assert_eq!(tables(&d), []);
}

#[test]
fn two_tables_of_one_output_are_told_apart_by_key() {
    let d = diagram(
        VARS,
        "y := a AND b OR c AND d;\nf := a AND b OR c AND d;\ny := y AND e OR NOT y AND f;",
    );
    let keys: Vec<String> = tables(&d).iter().map(|f| f.key.clone()).collect();
    assert_eq!(keys, ["table:Y:0", "table:F:0", "table:Y:1"]);
    applied(&d, &tables(&d));
}

/// All of `inputs`' settings through the scan: the value of the table's
/// output through the fold, the unfolded OR, and the row logic read off
/// the cells.
#[test]
fn the_values_through_the_fold_are_the_unfolded_diagrams_for_every_input() {
    let src = prog(
        "a AT %IX0.0 : BOOL; b AT %IX0.1 : BOOL; c AT %IX0.2 : BOOL; d AT %IX0.3 : BOOL; y, z : BOOL;",
        "y := (a AND NOT b) OR (c AND d) OR NOT a AND d;\nz := y;",
    );
    let (mut p, m) = build(&src);
    let d = &m.program().unwrap().diagram;
    let fs = tables(d);
    let v = applied(d, &fs);
    let fold = fold_block(&v).id;
    let table = fs[0].table.as_ref().unwrap();
    assert_eq!(table.rows.len(), 3);
    for inputs in 0..16u8 {
        let mut img = Image::new(SIZES);
        img.i[0] = inputs;
        let (r, t) = p.scan_traced(&mut img, Duration::from_millis(10));
        r.unwrap();
        let (a, b, c, dd) = (inputs & 1 != 0, inputs & 2 != 0, inputs & 4 != 0, inputs & 8 != 0);
        let want = (a && !b) || (c && dd) || (!a && dd);
        let live = annotate(d, &t, "");
        let seen = v.live(&live);
        // The block's output is the OR's, and the unfolded OR's.
        let out = seen.values[fold.0 as usize][0];
        assert_eq!(out, live.values[table.out.0 as usize][0], "inputs {inputs:04b}");
        assert_eq!(out, Some(Value::Bool(want)), "inputs {inputs:04b}");
        assert_eq!(t.get("Y"), Some(Value::Bool(want)));
        // The rows read off the cells' signals say the same.
        let holds = |row: &Vec<helmsman_diagram::fold::FoldCell>| {
            row.iter().all(|cell| {
                let (n, o) = cell.from;
                live.values[n.0 as usize][o] == Some(Value::Bool(!cell.negated))
            })
        };
        assert_eq!(table.rows.iter().any(holds), want, "inputs {inputs:04b}");
        // Every signal of a cell is still shown by the view, as a pin.
        for row in &table.rows {
            for cell in row {
                assert!(v.port(cell.from.0, cell.from.1).is_some());
            }
        }
    }
}

/// A table of `k` rows `s{i} AND NOT s{i+1}` over signals of long names.
fn long(k: usize) -> (Diagram, String) {
    let names: Vec<String> = (0..=k)
        .map(|i| format!("a_very_long_interlock_signal_name_{i}"))
        .collect();
    let vars = format!("{} : BOOL; y : BOOL;", names.join(", "));
    let mut body = String::from("y := ");
    let terms: Vec<String> = (0..k)
        .map(|i| format!("({} AND NOT {})", names[i], names[i + 1]))
        .collect();
    write!(body, "{};", terms.join(" OR ")).unwrap();
    (diagram(&vars, &body), body)
}

#[test]
fn many_rows_make_the_block_taller_and_long_names_never_wider() {
    let (d3, _) = long(3);
    let (d30, _) = long(30);
    let block = |d: &Diagram| {
        let v = applied(d, &tables(d));
        (fold_block(&v).clone(), v)
    };
    let (small, _) = block(&d3);
    let (big, v) = block(&d30);
    assert!(big.rect.h >= HEADER + 30.0 * PORT_ROW, "{:?}", big.rect);
    assert!(big.rect.h > small.rect.h);
    // The row is sized for names cut to CELL_CHARS and capped.
    assert!(
        big.rect.w <= small.rect.w + 40.0,
        "{:?} vs {:?}",
        big.rect,
        small.rect
    );
    let cap = (TABLE_CHARS as f32) * layout::CHAR_W;
    let names: f32 = big.ins.iter().map(|s| s.len() as f32).fold(0.0, f32::max) * layout::CHAR_W;
    assert!(big.rect.w <= 2.0 * names + cap + 80.0, "{:?}", big.rect);
    // Every row has room, and its cells split it without overlap, inside
    // the block.
    let rows = tables(&d30)[0].table.clone().unwrap().rows;
    assert_eq!(rows.len(), 30);
    for (k, row) in rows.iter().enumerate() {
        let cells: Vec<_> = (0..row.len())
            .map(|j| layout::body_cell(&big, k, j, row.len()))
            .collect();
        let r = layout::body_row(&big, k);
        assert!(r.w > 0.0 && r.h > 8.0, "{r:?}");
        for (j, c) in cells.iter().enumerate() {
            assert!(c.x >= r.x - 0.01 && c.right() <= r.right() + 0.01, "{c:?} {r:?}");
            assert!(
                c.y >= big.rect.y + HEADER && c.bottom() <= big.rect.bottom(),
                "{c:?}"
            );
            if let Some(next) = cells.get(j + 1) {
                assert!(c.right() <= next.x, "{c:?} {next:?}");
            }
        }
        if let Some(next) = rows.get(k + 1).map(|_| layout::body_row(&big, k + 1)) {
            assert!(r.bottom() <= next.y);
        }
    }
    // The widest row the layout is asked for is capped.
    let t = tables(&d30)[0].table.clone().unwrap();
    assert!(t.chars() <= TABLE_CHARS);
    assert!(t.chars() >= 2 * CELL_CHARS, "two long names fill their cells");
    check(&v.diagram);
}

#[test]
fn the_fold_is_deterministic_and_keyed_by_its_output() {
    let d = diagram(VARS, "y := a AND b OR c AND d;");
    assert_eq!(tables(&d), tables(&d));
    assert_eq!(view(&d, &tables(&d)), view(&d, &tables(&d)));
}
