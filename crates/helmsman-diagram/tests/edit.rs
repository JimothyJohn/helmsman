//! `helmsman_diagram::edit`: a gesture on the diagram is an exact splice of
//! the source. The property tests over generated programs are in
//! `tests/props.rs`; these are the shapes by hand, the refusals and the
//! awkward texts (comments between every token, CRLF, non-ASCII comments,
//! mixed case).

mod common;

use common::{build, prog};
use helmsman_diagram::edit::{Bubble, Edit, Literal, Pin, Refusal, Source, apply};
use helmsman_diagram::{Module, NodeId, NodeKind, RegionId, Span};
use helmsman_iec::ast::BinOp;

const VARS: &str = "a, b, c : INT; x, y, z : BOOL; tm : TON; t1, t2, t3 : TIME; r : REAL; wd : WORD;";

fn src(body: &str) -> String {
    prog(VARS, body)
}

fn node_with(m: &Module, text: &str) -> NodeId {
    let d = &m.program().unwrap().diagram;
    d.nodes
        .iter()
        .find(|n| n.text == text && !matches!(n.kind, NodeKind::Write { .. }))
        .unwrap_or_else(|| panic!("no block computing {text}"))
        .id
}

fn pin(node: NodeId, input: usize) -> Pin {
    Pin {
        pou: "t".into(),
        node,
        input,
    }
}

/// The source after `make`'s edit, or why it was refused.
fn run(source: &str, make: impl FnOnce(&Module) -> Edit) -> Result<String, Refusal> {
    let (_, m) = build(source);
    let patch = apply(source, &m, make(&m))?;
    let out = patch.apply_to(source);
    assert_eq!(
        patch.inverse(source).apply_to(&out),
        source,
        "the inverse patch restores the original bytes"
    );
    Ok(out)
}

fn toggle(source: &str, text: &str, input: usize) -> Result<String, Refusal> {
    run(source, |m| {
        Edit::ToggleNegation(Bubble::Pin(pin(node_with(m, text), input)))
    })
}

fn swap(source: &str, text: &str, to: BinOp) -> Result<String, Refusal> {
    run(source, |m| Edit::SwapOp {
        pou: "T".into(),
        node: node_with(m, text),
        to,
    })
}

fn set(source: &str, consumer: &str, input: usize, value: Literal) -> Result<String, Refusal> {
    run(source, |m| Edit::SetConst {
        pin: pin(node_with(m, consumer), input),
        value,
    })
}

#[test]
fn a_bubble_is_a_not_put_in_front_of_the_operand_and_taken_off_again() {
    let s = src("x := y AND z;");
    let on = toggle(&s, "Y AND Z", 1).unwrap();
    assert_eq!(on, src("x := y AND NOT z;"));
    let off = toggle(&on, "Y AND NOT Z", 1).unwrap();
    assert_eq!(off, s, "toggling twice is the original text");
}

#[test]
fn brackets_go_round_a_bare_comparison_and_come_off_with_the_not() {
    let s = src("x := a > 1 OR y;");
    let on = toggle(&s, "A > 1 OR Y", 0).unwrap();
    assert_eq!(on, src("x := NOT (a > 1) OR y;"));
    assert_eq!(toggle(&on, "NOT (A > 1) OR Y", 0).unwrap(), s);
    // Where a comparison is not a valid operand without them, they stay.
    let s = src("x := y AND NOT (a > 1 OR z);");
    let off = toggle(&s, "Y AND NOT (A > 1 OR Z)", 1).unwrap();
    assert_eq!(off, src("x := y AND (a > 1 OR z);"));
    // Brackets the user wrote round a plain name are left alone.
    let s = src("x := y AND NOT (z);");
    assert_eq!(toggle(&s, "Y AND NOT Z", 1).unwrap(), src("x := y AND (z);"));
}

#[test]
fn the_bubble_on_a_write_and_on_a_function_block_input() {
    let s = src("x := y;");
    let m = build(&s).1;
    let w = m
        .program()
        .unwrap()
        .diagram
        .nodes
        .iter()
        .find(|n| matches!(n.kind, NodeKind::Write { .. }))
        .unwrap()
        .id;
    let patch = apply(&s, &m, Edit::ToggleNegation(Bubble::Pin(pin(w, 0)))).unwrap();
    assert_eq!(patch.apply_to(&s), src("x := NOT y;"));
    let s = src("tm(IN := x, PT := T#5s);");
    assert_eq!(
        toggle(&s, "TM(IN := X, PT := T#5s)", 0).unwrap(),
        src("tm(IN := NOT x, PT := T#5s);")
    );
}

#[test]
fn the_bubble_on_a_gate() {
    for (head, head_not) in [
        ("IF a > 1 THEN", "IF NOT (a > 1) THEN"),
        ("IF x THEN", "IF NOT x THEN"),
        ("WHILE y DO", "WHILE NOT y DO"),
    ] {
        let end = if head.starts_with("WHILE") {
            "END_WHILE;"
        } else {
            "END_IF;"
        };
        let s = src(&format!("{head}\nb := 1;\n{end}"));
        let (_, m) = build(&s);
        let toggled = |s: &str, m: &Module| {
            let patch = apply(
                s,
                m,
                Edit::ToggleNegation(Bubble::Gate {
                    pou: "T".into(),
                    region: RegionId(0),
                }),
            )
            .unwrap();
            patch.apply_to(s)
        };
        let on = toggled(&s, &m);
        assert_eq!(on, src(&format!("{head_not}\nb := 1;\n{end}")));
        assert_eq!(toggled(&on, &build(&on).1), s);
    }
}

#[test]
fn a_literal_takes_a_new_value_in_the_same_place() {
    let s = src("a := b + 5;");
    assert_eq!(
        set(&s, "B + 5", 1, Literal::Int(12)).unwrap(),
        src("a := b + 12;")
    );
    assert_eq!(
        set(&s, "B + 5", 1, Literal::Int(-3)).unwrap(),
        src("a := b + -3;")
    );
    let s = src("a := b -5;");
    assert_eq!(
        set(&s, "B - 5", 1, Literal::Int(-3)).unwrap(),
        src("a := b - -3;")
    );
    let s = src("x := y AND TRUE;");
    assert_eq!(
        set(&s, "Y AND TRUE", 1, Literal::Bool(false)).unwrap(),
        src("x := y AND FALSE;")
    );
    let s = src("r := 1.5 * r;");
    assert_eq!(
        set(&s, "1.5 * R", 0, Literal::Real(2.0)).unwrap(),
        src("r := 2.0 * r;")
    );
}

#[test]
fn a_time_preset_is_set_in_its_largest_unit() {
    let s = src("tm(IN := x, PT := T#5s);");
    let call = "TM(IN := X, PT := T#5s)";
    assert_eq!(
        set(&s, call, 1, Literal::Time(90_000_000_000)).unwrap(),
        src("tm(IN := x, PT := T#90s);")
    );
    assert_eq!(
        set(&s, call, 1, Literal::Time(7_200_000_000_000)).unwrap(),
        src("tm(IN := x, PT := T#2h);")
    );
    // The letter case the user wrote, a type prefix and a sign are kept or
    // replaced with the number.
    let s = src("tm(IN := x, PT := t#5s);");
    assert_eq!(
        set(&s, "TM(IN := X, PT := T#5s)", 1, Literal::Time(250_000_000)).unwrap(),
        src("tm(IN := x, PT := t#250ms);")
    );
    let s = src("a := INT#5 + b;");
    assert_eq!(
        set(&s, "INT#5 + B", 0, Literal::Int(9)).unwrap(),
        src("a := INT#9 + b;")
    );
    let s = src("a := - (* c *) 7 + b;");
    assert_eq!(set(&s, "-7 + B", 0, Literal::Int(2)).unwrap(), src("a := 2 + b;"));
    let s = src("a := ( (* keep *) 7 ) + b;");
    assert_eq!(
        set(&s, "7 + B", 0, Literal::Int(2)).unwrap(),
        src("a := ( (* keep *) 2 ) + b;")
    );
}

#[test]
fn a_swapped_operator_keeps_its_operands_grouped_as_they_were() {
    // Same precedence level: nothing to add.
    let s = src("x := y AND z;");
    assert_eq!(swap(&s, "Y AND Z", BinOp::Or).unwrap(), src("x := y OR z;"));
    let s = src("x := a < b;");
    assert_eq!(swap(&s, "A < B", BinOp::Ge).unwrap(), src("x := a >= b;"));
    // An operand that would regroup gets brackets...
    let s = src("x := y OR z AND x;");
    assert_eq!(
        swap(&s, "Y OR Z AND X", BinOp::And).unwrap(),
        src("x := y AND (z AND x);")
    );
    let s = src("a := b + c * a;");
    assert_eq!(
        swap(&s, "B + C * A", BinOp::Mul).unwrap(),
        src("a := b * (c * a);")
    );
    // ...and so does the swapped expression in its parent.
    let s = src("x := y AND z AND x;");
    assert_eq!(
        swap(&s, "Y AND Z", BinOp::Or).unwrap(),
        src("x := (y OR z) AND x;")
    );
    let s = src("a := b - c + a;");
    assert_eq!(
        swap(&s, "B - C + A", BinOp::Mul).unwrap(),
        src("a := (b - c) * a;")
    );
    // A higher level over a lower one: the lower one needs them.
    let s = src("a := (b + c) * a;");
    assert_eq!(
        swap(&s, "(B + C) * A", BinOp::Sub).unwrap(),
        src("a := (b + c) - a;")
    );
    // Under a NOT bubble the operator is already in brackets.
    let s = src("x := NOT (y AND z);");
    assert_eq!(swap(&s, "Y AND Z", BinOp::Or).unwrap(), src("x := NOT (y OR z);"));
}

#[test]
fn keywords_keep_the_case_they_were_written_in() {
    let s = src("x := y and z;");
    assert_eq!(swap(&s, "Y AND Z", BinOp::Or).unwrap(), src("x := y or z;"));
    let s = src("x := y & z;");
    assert_eq!(swap(&s, "Y AND Z", BinOp::Or).unwrap(), src("x := y OR z;"));
    let s = src("x := y AND z;");
    assert_eq!(toggle(&s, "Y AND Z", 0).unwrap(), src("x := NOT y AND z;"));
}

#[test]
fn every_edit_survives_a_comment_between_every_token() {
    let plain = "x := y AND NOT z OR a > 1;\ntm(IN := x, PT := T#5s);\nIF x AND y THEN b := 1; END_IF;";
    let noisy = plain.replace(' ', " (* c *) ");
    let s = src(&noisy);
    let (_, m) = build(&s);
    let d = &m.program().unwrap().diagram;
    let comments = s.matches("(* c *)").count();
    let mut done = 0;
    for n in &d.nodes {
        for input in 0..n.ins.len() {
            let p = pin(n.id, input);
            let kinds = [
                Edit::ToggleNegation(Bubble::Pin(p.clone())),
                Edit::SetConst {
                    pin: p,
                    value: Literal::Int(4),
                },
            ];
            for e in kinds {
                if let Ok(patch) = apply(&s, &m, e) {
                    let out = patch.apply_to(&s);
                    assert_eq!(
                        out.matches("(* c *)").count(),
                        comments,
                        "no comment is lost: {out}"
                    );
                    assert_eq!(patch.inverse(&s).apply_to(&out), s);
                    done += 1;
                }
            }
        }
        if matches!(&n.kind, NodeKind::Op { op } if op == "AND" || op == "OR" || op == ">") {
            let to = if n.label == ">" { BinOp::Lt } else { BinOp::Or };
            let to = if n.label == "OR" { BinOp::And } else { to };
            let patch = apply(
                &s,
                &m,
                Edit::SwapOp {
                    pou: "T".into(),
                    node: n.id,
                    to,
                },
            )
            .unwrap();
            let out = patch.apply_to(&s);
            assert_eq!(out.matches("(* c *)").count(), comments);
            done += 1;
        }
    }
    assert!(done > 8, "{done} edits made");
}

#[test]
fn crlf_mixed_case_and_non_ascii_comments_do_not_move_the_patch() {
    let body = "(* ünïcödé ✓ *) X := y AnD (* µ *) NOT Z; // ✓ done\r\nIF Y THEN (* ✓ *) b := 5; END_IF;\r\n";
    let s = src(body).replace('\n', "\r\n").replace("\r\r\n", "\r\n");
    let (_, m) = build(&s);
    let d = &m.program().unwrap().diagram;
    let and = d.nodes.iter().find(|n| n.label == "AND").unwrap().id;
    let patch = apply(
        &s,
        &m,
        Edit::SwapOp {
            pou: "T".into(),
            node: and,
            to: BinOp::Or,
        },
    )
    .unwrap();
    let out = patch.apply_to(&s);
    assert_eq!(out, s.replace("AnD", "OR"), "{out:?}");
    let patch = apply(&s, &m, Edit::ToggleNegation(Bubble::Pin(pin(and, 1)))).unwrap();
    assert_eq!(patch.apply_to(&s), s.replace("NOT Z", "Z"));
    let patch = apply(&s, &m, Edit::ToggleNegation(Bubble::Pin(pin(and, 0)))).unwrap();
    assert_eq!(patch.apply_to(&s), s.replace("y AnD", "NOT y AnD"));
    let five = d.nodes.iter().find(|n| n.text == "5").unwrap().id;
    let w = d.wires.iter().find(|w| w.from == five).unwrap();
    let patch = apply(
        &s,
        &m,
        Edit::SetConst {
            pin: pin(w.to, w.inp),
            value: Literal::Int(6),
        },
    )
    .unwrap();
    assert_eq!(patch.apply_to(&s), s.replace("b := 5", "b := 6"));
    assert!(s.is_char_boundary(patch.start) && s.is_char_boundary(patch.end));
}

#[test]
fn refusals_say_why() {
    let s = src(
        "x := y AND z;\na := b + 1;\nwd := NOT wd AND 16#FF;\nt3 := t1 + t2;\nIF x THEN b := 2; ELSE b := 3; END_IF;",
    );
    let (_, m) = build(&s);
    let and = node_with(&m, "Y AND Z");
    let say = |e: Edit| apply(&s, &m, e).unwrap_err();
    let t = |n: NodeId, i: usize| Edit::ToggleNegation(Bubble::Pin(pin(n, i)));

    assert_eq!(say(t(and, 5)), Refusal::NoSuchInput);
    assert_eq!(say(t(NodeId(9999), 0)), Refusal::NoSuchBlock);
    assert_eq!(
        say(Edit::ToggleNegation(Bubble::Pin(Pin {
            pou: "nope".into(),
            node: and,
            input: 0
        }))),
        Refusal::NoSuchPou("nope".into())
    );
    // A literal has no bubble; a variable is no value.
    let plus = node_with(&m, "B + 1");
    assert_eq!(say(t(plus, 1)), Refusal::NoBubble);
    assert!(matches!(
        say(Edit::SetConst {
            pin: pin(and, 0),
            value: Literal::Bool(true)
        }),
        Refusal::NotAValue(_)
    ));
    // NOT on a WORD is a block.
    let wand = node_with(&m, "NOT WD AND 255");
    assert_eq!(say(t(wand, 0)), Refusal::NotBoolean);
    // A number is no condition.
    let r = say(t(plus, 0));
    assert!(
        matches!(r, Refusal::NotBoolean | Refusal::WouldNotCompile(_)),
        "{r:?}"
    );
}

#[test]
fn refusals_for_operators_regions_and_values_say_why() {
    let s = src(
        "x := y AND z;\na := b + 1;\nwd := NOT wd AND 16#FF;\nt3 := t1 + t2;\nIF x THEN b := 2; ELSE b := 3; END_IF;",
    );
    let (_, m) = build(&s);
    let and = node_with(&m, "Y AND Z");
    let say = |e: Edit| apply(&s, &m, e).unwrap_err();
    let plus = node_with(&m, "B + 1");
    assert_eq!(
        say(Edit::SwapOp {
            pou: "T".into(),
            node: and,
            to: BinOp::Add
        }),
        Refusal::WrongFamily {
            from: "AND".into(),
            to: "+".into()
        }
    );
    assert_eq!(
        say(Edit::SwapOp {
            pou: "T".into(),
            node: and,
            to: BinOp::And
        }),
        Refusal::NothingToChange
    );
    let konst = node_with(&m, "1");
    assert_eq!(
        say(Edit::SwapOp {
            pou: "T".into(),
            node: konst,
            to: BinOp::And
        }),
        Refusal::NotAnOperator
    );
    let times = node_with(&m, "T1 + T2");
    let r = say(Edit::SwapOp {
        pou: "T".into(),
        node: times,
        to: BinOp::Mul,
    });
    assert!(matches!(r, Refusal::WouldNotCompile(_)), "TIME * TIME: {r:?}");
    // Regions without a condition.
    let els = m
        .program()
        .unwrap()
        .diagram
        .regions
        .iter()
        .find(|r| r.label == "ELSE")
        .unwrap()
        .id;
    assert_eq!(
        say(Edit::ToggleNegation(Bubble::Gate {
            pou: "T".into(),
            region: els
        })),
        Refusal::NoCondition
    );
    assert_eq!(
        say(Edit::ToggleNegation(Bubble::Gate {
            pou: "T".into(),
            region: RegionId(77)
        })),
        Refusal::NoSuchRegion
    );
    // A value that cannot be written.
    assert!(matches!(
        say(Edit::SetConst {
            pin: pin(plus, 1),
            value: Literal::Real(f64::NAN)
        }),
        Refusal::BadValue(_)
    ));
    // Not a change at all.
    assert_eq!(
        say(Edit::SetConst {
            pin: pin(plus, 1),
            value: Literal::Int(1)
        }),
        Refusal::NothingToChange
    );
    // The words are plain.
    assert!(Refusal::NotBoolean.to_string().contains("bitwise complement"));
}

#[test]
fn a_function_block_output_has_no_text_of_its_own() {
    let s = src("tm(IN := x, PT := T#5s, ET => t1);\nx := tm.Q;");
    let (_, m) = build(&s);
    let d = &m.program().unwrap().diagram;
    let w = d.nodes.iter().find(|n| n.label == "T1 :=").unwrap().id;
    assert_eq!(
        apply(&s, &m, Edit::ToggleNegation(Bubble::Pin(pin(w, 0)))).unwrap_err(),
        Refusal::NoText
    );
}

#[test]
fn a_diagram_from_other_text_is_refused_not_applied() {
    let a = src("x := y AND z;");
    let b = src("x := y AND z; (* a comment moves every byte *)\nb := 1;");
    let c = src("x := y OR z;");
    let (_, ma) = build(&a);
    let (_, mb) = build(&b);
    for (text, module) in [(&b, &ma), (&a, &mb), (&c, &ma), (&a, &mb)] {
        let and = module
            .program()
            .unwrap()
            .diagram
            .nodes
            .iter()
            .find(|n| n.label == "AND" || n.label == "OR")
            .unwrap()
            .id;
        for e in [
            Edit::SwapOp {
                pou: "T".into(),
                node: and,
                to: BinOp::Or,
            },
            Edit::ToggleNegation(Bubble::Pin(pin(and, 0))),
        ] {
            assert_eq!(apply(text, module, e).unwrap_err(), Refusal::StaleDiagram);
        }
    }
    // The same text, laid out differently, is the same diagram.
    let mut moved = ma.clone();
    for n in &mut moved.pous[0].diagram.nodes {
        n.rect.x += 100.0;
    }
    let and = node_with(&ma, "Y AND Z");
    assert!(
        apply(
            &a,
            &moved,
            Edit::SwapOp {
                pou: "T".into(),
                node: and,
                to: BinOp::Or
            }
        )
        .is_ok()
    );
}

#[test]
fn a_source_that_does_not_compile_is_refused() {
    let good = src("x := y AND z;");
    let (_, m) = build(&good);
    let bad = src("x := y AND ;");
    let and = node_with(&m, "Y AND Z");
    let r = apply(
        &bad,
        &m,
        Edit::SwapOp {
            pou: "T".into(),
            node: and,
            to: BinOp::Or,
        },
    )
    .unwrap_err();
    assert!(matches!(r, Refusal::SourceDoesNotCompile(_)), "{r:?}");
}

#[test]
fn only_the_addressed_pou_changes() {
    let s = "FUNCTION_BLOCK f\nVAR_INPUT i : BOOL; END_VAR\nVAR_OUTPUT q : BOOL; END_VAR\nq := i AND NOT i;\nEND_FUNCTION_BLOCK\n\
PROGRAM t\nVAR x, y : BOOL; END_VAR\nx := y AND y;\nEND_PROGRAM\n";
    let (_, m) = build(s);
    let f = m.pou("f").unwrap();
    let and = f.diagram.nodes.iter().find(|n| n.label == "AND").unwrap().id;
    let patch = apply(
        s,
        &m,
        Edit::SwapOp {
            pou: "F".into(),
            node: and,
            to: BinOp::Or,
        },
    )
    .unwrap();
    assert_eq!(
        patch.apply_to(s),
        s.replace("q := i AND NOT i", "q := i OR NOT i")
    );
}

// ---- rewire, SetFbArg, DeleteStmt, Rename ---------------------------------

fn write_of(m: &Module, label: &str) -> NodeId {
    m.program()
        .unwrap()
        .diagram
        .nodes
        .iter()
        .find(|n| n.label == label && matches!(n.kind, NodeKind::Write { .. }))
        .unwrap_or_else(|| panic!("no write {label}"))
        .id
}

fn rewire(source: &str, consumer: &str, input: usize, to: Source) -> Result<String, Refusal> {
    run(source, |m| Edit::Rewire {
        pin: pin(node_with(m, consumer), input),
        to,
    })
}

fn var(s: &str) -> Source {
    Source::Var(s.into())
}

fn mismatch(e: &Refusal) -> bool {
    matches!(e, Refusal::TypeMismatch { .. })
}

#[test]
fn a_pin_is_wired_to_a_variable_or_a_literal_in_place() {
    let s = src("x := y AND z;");
    assert_eq!(rewire(&s, "Y AND Z", 1, var("x")).unwrap(), src("x := y AND x;"));
    assert_eq!(
        rewire(&s, "Y AND Z", 0, Source::Const(Literal::Bool(true))).unwrap(),
        src("x := TRUE AND z;")
    );
    let s = src("a := b + 5;");
    assert_eq!(rewire(&s, "B + 5", 1, var("c")).unwrap(), src("a := b + c;"));
    assert_eq!(
        rewire(&s, "B + 5", 0, Source::Const(Literal::Int(-3))).unwrap(),
        src("a := -3 + 5;")
    );
    // The same variable already read in the statement: one block, then two.
    let s = src("a := b + b;");
    assert_eq!(rewire(&s, "B + B", 1, var("c")).unwrap(), src("a := b + c;"));
    let s = src("a := b + c;");
    assert_eq!(rewire(&s, "B + C", 1, var("b")).unwrap(), src("a := b + b;"));
    // A member of a function block.
    let s = src("x := y AND z;");
    assert_eq!(
        rewire(&s, "Y AND Z", 1, var("tm.Q")).unwrap(),
        src("x := y AND tm.Q;")
    );
    assert!(mismatch(
        &rewire(&src("a := b + 1;"), "B + 1", 0, var("t1")).unwrap_err()
    ));
}

#[test]
fn a_bubble_stays_on_the_pin_and_does_not_go_with_a_literal() {
    let s = src("x := y AND NOT z;");
    assert_eq!(
        rewire(&s, "Y AND NOT Z", 1, var("x")).unwrap(),
        src("x := y AND NOT x;")
    );
    assert_eq!(
        rewire(&s, "Y AND NOT Z", 1, Source::Const(Literal::Bool(false))).unwrap(),
        src("x := y AND FALSE;")
    );
    let s = src("x := y AND NOT (z);");
    assert_eq!(
        rewire(&s, "Y AND NOT Z", 1, var("x")).unwrap(),
        src("x := y AND NOT x;")
    );
}

#[test]
fn a_negative_literal_does_not_fuse_with_a_minus() {
    let s = src("a := b -c;");
    assert_eq!(
        rewire(&s, "B - C", 1, Source::Const(Literal::Int(-2))).unwrap(),
        src("a := b - -2;")
    );
}

#[test]
fn a_variable_is_written_as_it_is_declared_whatever_case_is_given() {
    let s = "PROGRAM t\nVAR Start, StopBtn, Run : BOOL; END_VAR\nRun := Start AND StopBtn;\nEND_PROGRAM\n";
    let (_, m) = build(s);
    let and = m
        .program()
        .unwrap()
        .diagram
        .nodes
        .iter()
        .find(|n| n.label == "AND")
        .unwrap()
        .id;
    for given in ["start", "START", "sTaRt"] {
        let patch = apply(
            s,
            &m,
            Edit::Rewire {
                pin: pin(and, 1),
                to: var(given),
            },
        )
        .unwrap();
        assert_eq!(
            patch.apply_to(s),
            s.replace("Start AND StopBtn", "Start AND Start")
        );
    }
    let patch = apply(
        s,
        &m,
        Edit::Rewire {
            pin: pin(and, 0),
            to: var("RUN"),
        },
    )
    .unwrap();
    assert_eq!(patch.apply_to(s), s.replace("Run := Start", "Run := Run"));
}

#[test]
fn a_variable_of_a_later_section_is_there_one_of_another_pou_or_nowhere_is_not() {
    // The declarations come before the statements, so "later" is a later
    // section of the POU.
    let s = "FUNCTION_BLOCK f\nVAR_INPUT i : BOOL; END_VAR\nVAR_OUTPUT q : BOOL; END_VAR\nVAR hidden : BOOL; END_VAR\nq := i;\nEND_FUNCTION_BLOCK\n\
PROGRAM t\nVAR x, y : BOOL; END_VAR\nVAR_TEMP late : BOOL; END_VAR\nx := y AND y;\nEND_PROGRAM\n";
    let (_, m) = build(s);
    let and = m
        .program()
        .unwrap()
        .diagram
        .nodes
        .iter()
        .find(|n| n.label == "AND")
        .unwrap()
        .id;
    let go = |name: &str| {
        apply(
            s,
            &m,
            Edit::Rewire {
                pin: pin(and, 1),
                to: var(name),
            },
        )
        .map(|p| p.apply_to(s))
    };
    assert_eq!(go("late").unwrap(), s.replace("y AND y", "y AND late"));
    assert_eq!(
        go("hidden").unwrap_err(),
        Refusal::NoSuchVariable {
            name: "HIDDEN".into(),
            pou: "T".into()
        }
    );
    assert_eq!(
        go("nowhere").unwrap_err().to_string(),
        "there is no variable called NOWHERE in T"
    );
    // Text that is not a variable at all.
    for bad in ["", "y AND z", "1", "(y)", " y", "y ", "NOT y", "y (* c *)"] {
        assert!(
            matches!(go(bad).unwrap_err(), Refusal::NotAVariable(_)),
            "{bad:?}: {:?}",
            go(bad)
        );
    }
}

#[test]
fn a_source_whose_type_does_not_fit_is_refused_by_the_programs_types() {
    let s = src("x := y AND z;\na := b + c;\ntm(IN := x, PT := T#5s);");
    let say = |consumer: &str, input: usize, to: Source| rewire(&s, consumer, input, to).unwrap_err();
    assert!(mismatch(&say("Y AND Z", 0, var("a"))), "an INT into a BOOL input");
    assert!(mismatch(&say("B + C", 0, var("x"))), "a BOOL into an INT operand");
    assert!(
        mismatch(&say("B + C", 0, var("t1"))),
        "a TIME into an INT operand"
    );
    assert!(mismatch(&say("B + C", 0, Source::Const(Literal::Bool(true)))));
    assert!(mismatch(&say("Y AND Z", 0, Source::Const(Literal::Int(1)))));
    let call = "TM(IN := X, PT := T#5s)";
    assert!(
        mismatch(&say(call, 1, Source::Const(Literal::Int(5)))),
        "5 is no TIME"
    );
    assert!(mismatch(&say(call, 1, var("a"))));
    assert!(mismatch(&say(call, 0, var("a"))));
    assert_eq!(
        rewire(&s, call, 1, var("t2")).unwrap(),
        s.replace("PT := T#5s", "PT := t2")
    );
    // A pin with a bubble takes only a BOOL.
    let s = src("x := y AND NOT z;");
    assert!(mismatch(&rewire(&s, "Y AND NOT Z", 1, var("a")).unwrap_err()));
    assert_eq!(
        Refusal::TypeMismatch {
            expected: "BOOL".into(),
            found: "INT".into(),
        }
        .to_string(),
        "the types do not fit: that input takes BOOL and the new source is INT"
    );
}

#[test]
fn only_a_pin_fed_by_a_variable_a_value_or_a_write_is_rewired() {
    let s = src("x := y AND (z OR x);\na := ABS(b) + 1;\nx := tm.Q AND y;");
    let (_, m) = build(&s);
    let and = node_with(&m, "Y AND (Z OR X)");
    let r = apply(
        &s,
        &m,
        Edit::Rewire {
            pin: pin(and, 1),
            to: var("x"),
        },
    )
    .unwrap_err();
    assert_eq!(r, Refusal::NotALeaf("the operator OR".into()));
    assert!(r.to_string().contains("change the inputs of that block"));
    let plus = node_with(&m, "ABS(B) + 1");
    let r = apply(
        &s,
        &m,
        Edit::Rewire {
            pin: pin(plus, 0),
            to: var("a"),
        },
    )
    .unwrap_err();
    assert_eq!(r, Refusal::NotALeaf("the call of ABS".into()));
    // An FB output is a source like any other.
    let and2 = node_with(&m, "TM.Q AND Y");
    assert!(
        apply(
            &s,
            &m,
            Edit::Rewire {
                pin: pin(and2, 0),
                to: var("z")
            }
        )
        .is_ok()
    );
}

#[test]
fn an_earlier_write_is_a_source_only_where_it_is_still_the_last_one() {
    let to_write = |s: &str, m: &Module, text: &str, w: NodeId| {
        apply(
            s,
            m,
            Edit::Rewire {
                pin: pin(node_with(m, text), 0),
                to: Source::Write(w),
            },
        )
        .map(|p| p.apply_to(s))
    };
    let s = src("a := b + 1;\nc := b * 2;");
    let (_, m) = build(&s);
    assert_eq!(
        to_write(&s, &m, "B * 2", write_of(&m, "A :=")).unwrap(),
        src("a := b + 1;\nc := a * 2;"),
        "the pin is now wired from the write"
    );
    // After the pin: the write is not there yet.
    let s2 = src("c := b * 2;\na := b + 1;");
    let (_, m2) = build(&s2);
    let r = to_write(&s2, &m2, "B * 2", write_of(&m2, "A :=")).unwrap_err();
    assert_eq!(r, Refusal::NotWrittenYet("A".into()));
    assert!(r.to_string().contains("not written at that point of the scan"));
    // Written again in between: the pin would read the second write.
    let s3 = src("a := b + 1;\na := 7;\nc := b * 2;");
    let (_, m3) = build(&s3);
    assert_eq!(
        to_write(&s3, &m3, "B * 2", write_of(&m3, "A :=")).unwrap_err(),
        Refusal::NotWrittenYet("A".into())
    );
    // In a branch the pin is not in.
    let s4 = src("IF x THEN\na := b + 1;\nELSE\nc := b * 2;\nEND_IF;");
    let (_, m4) = build(&s4);
    assert_eq!(
        to_write(&s4, &m4, "B * 2", write_of(&m4, "A :=")).unwrap_err(),
        Refusal::NotWrittenYet("A".into())
    );
    // In an enclosing region it is.
    let s5 = src("a := b + 1;\nIF x THEN\nc := b * 2;\nEND_IF;");
    let (_, m5) = build(&s5);
    assert_eq!(
        to_write(&s5, &m5, "B * 2", write_of(&m5, "A :=")).unwrap(),
        src("a := b + 1;\nIF x THEN\nc := a * 2;\nEND_IF;")
    );
    // A block that is not a write, and one that is not there.
    let times = node_with(&m, "B * 2");
    assert_eq!(to_write(&s, &m, "B * 2", times).unwrap_err(), Refusal::NotAWrite);
    assert_eq!(
        to_write(&s, &m, "B * 2", NodeId(999)).unwrap_err(),
        Refusal::NoSuchBlock
    );
}

fn fb_arg(source: &str, arg: &str, to: Source) -> Result<String, Refusal> {
    run(source, |m| {
        let node = m
            .program()
            .unwrap()
            .diagram
            .nodes
            .iter()
            .find(|n| matches!(n.kind, NodeKind::Fb { .. }))
            .unwrap()
            .id;
        Edit::SetFbArg {
            pou: "t".into(),
            node,
            arg: arg.into(),
            to,
        }
    })
}

#[test]
fn a_function_block_argument_is_replaced_or_added() {
    let time = |ns| Source::Const(Literal::Time(ns));
    let s = src("tm(IN := x, PT := T#5s);");
    assert_eq!(
        fb_arg(&s, "PT", time(7_000_000_000)).unwrap(),
        src("tm(IN := x, PT := T#7s);")
    );
    assert_eq!(
        fb_arg(&s, "pt", var("t2")).unwrap(),
        src("tm(IN := x, PT := t2);")
    );
    // Added after the last argument, as the declaration spells it.
    let s = src("tm(IN := x);");
    assert_eq!(
        fb_arg(&s, "pt", time(5_000_000_000)).unwrap(),
        src("tm(IN := x, PT := T#5s);")
    );
    let s = src("tm(PT := T#5s);");
    assert_eq!(
        fb_arg(&s, "in", var("y")).unwrap(),
        src("tm(PT := T#5s, IN := y);")
    );
    let s = src("tm();");
    assert_eq!(fb_arg(&s, "IN", var("y")).unwrap(), src("tm(IN := y);"));
    let s = src("tm ( (* none *) );");
    assert_eq!(
        fb_arg(&s, "IN", var("y")).unwrap(),
        src("tm (IN := y (* none *) );")
    );
    // A comment or a break after the last argument stays where it was.
    let s = src("tm(IN := x // note\n);");
    assert_eq!(
        fb_arg(&s, "PT", time(5_000_000_000)).unwrap(),
        src("tm(IN := x, PT := T#5s // note\n);")
    );
    let s = src("tm(IN := x (* c *) , ET => t1 );");
    assert_eq!(
        fb_arg(&s, "PT", time(5_000_000_000)).unwrap(),
        src("tm(IN := x, PT := T#5s (* c *) , ET => t1 );")
    );
    // Inside a branch, with CRLF.
    let s = src("IF y THEN\n  tm(IN := x);\nEND_IF;").replace('\n', "\r\n");
    assert_eq!(
        fb_arg(&s, "PT", time(1_000_000_000)).unwrap(),
        src("IF y THEN\n  tm(IN := x, PT := T#1s);\nEND_IF;").replace('\n', "\r\n")
    );
}

#[test]
fn function_block_argument_refusals_say_why() {
    let s = src("tm(IN := x);\na := b + 1;");
    let say = |arg: &str, to: Source| fb_arg(&s, arg, to).unwrap_err();
    assert_eq!(
        say("NOPE", var("y")),
        Refusal::NotAnInput {
            fb: "TON".into(),
            arg: "NOPE".into()
        }
    );
    // An output is no input.
    assert!(matches!(say("Q", var("y")), Refusal::NotAnInput { .. }));
    assert!(mismatch(&say("PT", Source::Const(Literal::Int(5)))));
    assert!(matches!(say("PT", var("zzz")), Refusal::NoSuchVariable { .. }));
    assert!(mismatch(&say("PT", var("b"))));
    assert_eq!(
        Refusal::NotAnInput {
            fb: "TON".into(),
            arg: "Q".into()
        }
        .to_string(),
        "TON has no input called Q"
    );
    // Not a call at all.
    let (_, m) = build(&s);
    let plus = node_with(&m, "B + 1");
    assert_eq!(
        apply(
            &s,
            &m,
            Edit::SetFbArg {
                pou: "t".into(),
                node: plus,
                arg: "IN".into(),
                to: var("x")
            }
        )
        .unwrap_err(),
        Refusal::NotAFunctionBlock
    );
}

fn delete(source: &str, text: &str) -> Result<String, Refusal> {
    let start = source.find(text).unwrap_or_else(|| panic!("no {text:?}"));
    run(source, |_| Edit::DeleteStmt {
        pou: "t".into(),
        span: Span {
            start,
            end: start + text.len(),
        },
    })
}

#[test]
fn a_statement_on_its_own_line_goes_with_the_line() {
    let s = src("a := 1;\n  b := 2; // two\nc := 3;");
    assert_eq!(delete(&s, "b := 2;").unwrap(), src("a := 1;\nc := 3;"));
    assert_eq!(delete(&s, "a := 1;").unwrap(), src("  b := 2; // two\nc := 3;"));
    assert_eq!(delete(&s, "c := 3;").unwrap(), src("a := 1;\n  b := 2; // two"));
    let s = src("a := 1;\n\tb := 2; (* two *)  \nc := 3;");
    assert_eq!(delete(&s, "b := 2;").unwrap(), src("a := 1;\nc := 3;"));
    // CRLF.
    let s = src("a := 1;\n  b := 2;\nc := 3;").replace('\n', "\r\n");
    assert_eq!(
        delete(&s, "b := 2;").unwrap(),
        src("a := 1;\nc := 3;").replace('\n', "\r\n")
    );
    // A comment of its own above the statement stays.
    let s = src("a := 1;\n// about b\nb := 2;");
    assert_eq!(delete(&s, "b := 2;").unwrap(), src("a := 1;\n// about b"));
}

#[test]
fn two_statements_on_a_line_lose_one_and_the_blanks_between() {
    let s = src("a := 1; b := 2;");
    assert_eq!(delete(&s, "a := 1;").unwrap(), src("b := 2;"));
    assert_eq!(delete(&s, "b := 2;").unwrap(), src("a := 1;"));
    let s = src("  a := 1;   b := 2; c := 3; // end");
    assert_eq!(delete(&s, "b := 2;").unwrap(), src("  a := 1;   c := 3; // end"));
    assert_eq!(delete(&s, "c := 3;").unwrap(), src("  a := 1;   b := 2; // end"));
    assert_eq!(delete(&s, "a := 1;").unwrap(), src("  b := 2; c := 3; // end"));
    let s = src("a := 1; (* about b *) b := 2;");
    assert_eq!(delete(&s, "a := 1;").unwrap(), src("(* about b *) b := 2;"));
}

#[test]
fn deleting_the_only_statement_of_a_branch_or_of_a_pou_leaves_valid_source() {
    let s = src("IF x THEN\n  a := 1;\nEND_IF;\nb := 2;");
    assert_eq!(delete(&s, "a := 1;").unwrap(), src("IF x THEN\nEND_IF;\nb := 2;"));
    let s = src("IF x THEN\n  a := 1;\nELSE\n  a := 2;\nEND_IF;");
    assert_eq!(
        delete(&s, "a := 2;").unwrap(),
        src("IF x THEN\n  a := 1;\nELSE\nEND_IF;")
    );
    let s = src("IF x THEN a := 1; END_IF;");
    assert_eq!(delete(&s, "a := 1;").unwrap(), src("IF x THEN END_IF;"));
    // The whole compound statement, with everything in it.
    let s = src("a := 0;\nIF x THEN\n  a := 1; (* in *)\n  b := 2;\nEND_IF;\nc := 3;");
    assert_eq!(
        delete(&s, "IF x THEN\n  a := 1; (* in *)\n  b := 2;\nEND_IF;").unwrap(),
        src("a := 0;\nc := 3;")
    );
    // The last statement of the POU, and its only one.
    let s = "PROGRAM t\nVAR a : INT; END_VAR\na := 1;\nEND_PROGRAM";
    assert_eq!(
        delete(s, "a := 1;").unwrap(),
        "PROGRAM t\nVAR a : INT; END_VAR\nEND_PROGRAM"
    );
    // A loop.
    let s = src("FOR b := 0 TO 3 DO\n  a := a + 1;\nEND_FOR;");
    assert_eq!(
        delete(&s, "a := a + 1;").unwrap(),
        src("FOR b := 0 TO 3 DO\nEND_FOR;")
    );
}

#[test]
fn a_delete_leaves_the_rest_reading_what_it_can_and_says_so_otherwise() {
    // The later statement read the deleted write; now it reads the variable.
    let s = src("a := b + 1;\nc := a * 2;");
    assert_eq!(delete(&s, "a := b + 1;").unwrap(), src("c := a * 2;"));
    // ...or an older write of it.
    let s = src("a := 5;\na := b + 1;\nc := a * 2;");
    assert_eq!(delete(&s, "a := b + 1;").unwrap(), src("a := 5;\nc := a * 2;"));
    // A function block call whose output is read afterwards.
    let s = src("tm(IN := x, PT := T#5s);\ny := tm.Q;");
    assert_eq!(delete(&s, "tm(IN := x, PT := T#5s);").unwrap(), src("y := tm.Q;"));
}

#[test]
fn a_range_that_is_not_one_statement_is_refused() {
    let s = src("a := 1;\nb := 2;\nIF x THEN c := 3; END_IF;");
    let here = |from: usize, to: usize| {
        let (_, m) = build(&s);
        apply(
            &s,
            &m,
            Edit::DeleteStmt {
                pou: "t".into(),
                span: Span { start: from, end: to },
            },
        )
        .unwrap_err()
    };
    let at = s.find("a := 1;").unwrap();
    assert_eq!(here(at, at + 6), Refusal::NotAStatement, "missing the semicolon");
    assert_eq!(here(at, at + 8), Refusal::NotAStatement, "one byte too far");
    assert_eq!(here(at + 1, at + 7), Refusal::NotAStatement, "starts inside");
    assert_eq!(
        here(at, at + "a := 1;\nb := 2;".len()),
        Refusal::NotAStatement,
        "two statements"
    );
    assert_eq!(here(at, at), Refusal::NotAStatement, "empty");
    let c = s.find("c := 3").unwrap();
    assert_eq!(here(c, c + "c := 3; END_IF;".len()), Refusal::NotAStatement);
    assert_eq!(here(0, s.len()), Refusal::NotAStatement, "the whole file");
    assert_eq!(
        here(s.len() + 5, s.len() + 9),
        Refusal::NotAStatement,
        "past the end"
    );
    let x = s.find("x THEN").unwrap();
    assert_eq!(here(x, x + 1), Refusal::NotAStatement, "an expression");
    assert!(
        Refusal::NotAStatement
            .to_string()
            .contains("exactly one statement")
    );
}
