//! `helmsman_diagram::edit`: a gesture on the diagram is an exact splice of
//! the source. The property tests over generated programs are in
//! `tests/props.rs`; these are the shapes by hand, the refusals and the
//! awkward texts (comments between every token, CRLF, non-ASCII comments,
//! mixed case).

mod common;

use common::{build, prog};
use helmsman_diagram::edit::{Bubble, Edit, Literal, Pin, Refusal, apply};
use helmsman_diagram::{Module, NodeId, NodeKind, RegionId};
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
