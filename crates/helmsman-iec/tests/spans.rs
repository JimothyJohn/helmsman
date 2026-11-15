//! Every statement and expression knows its exact source range: the text
//! from its start to its `end` parses back to the same tree. The property
//! test over generated programs is in helmsman-diagram's `tests/props.rs`;
//! these are the awkward shapes by hand.

mod spans_check;

use helmsman_iec::ast::{self, ExprKind, StmtKind};
use spans_check::{check, offset, stmt_text};

fn prog(body: &str) -> String {
    format!(
        "PROGRAM t\nVAR a, b, c : INT; x, y : BOOL; arr : ARRAY[0..3] OF INT; END_VAR\n{body}\nEND_PROGRAM\n"
    )
}

fn ok(body: &str) -> usize {
    let src = prog(body);
    check(&src).unwrap_or_else(|e| panic!("{e}\n{src}"))
}

#[test]
fn a_simple_statement_ends_after_its_semicolon() {
    let src = prog("a := b + 1;   (* trailing *)\nb := 2;");
    assert_eq!(stmt_text(&src, 0), "a := b + 1;");
    assert_eq!(stmt_text(&src, 1), "b := 2;");
    ok("a := b + 1;   (* trailing *)\nb := 2;");
}

#[test]
fn comments_between_tokens_stay_inside_and_outside() {
    let body = "a (* lhs *) := (* between *) b (* x *) + (* y *) 1 (* after *) ; (* tail *)\n// line\nb := 2; // end";
    let src = prog(body);
    assert_eq!(
        stmt_text(&src, 0),
        "a (* lhs *) := (* between *) b (* x *) + (* y *) 1 (* after *) ;"
    );
    ok(body);
}

#[test]
fn trailing_whitespace_is_outside() {
    let src = prog("a := 1;   \t \nb := 2 ;  ");
    assert_eq!(stmt_text(&src, 0), "a := 1;");
    assert_eq!(stmt_text(&src, 1), "b := 2 ;");
    ok("a := 1;   \t \nb := 2 ;  ");
}

#[test]
fn nested_parentheses_belong_to_the_expression() {
    let src = prog("a := ((b + 1)) * (c);");
    let unit = ast::parse(&src).unwrap();
    let StmtKind::Assign(_, rhs) = &unit.pous[0].body[0].kind else {
        panic!("assign")
    };
    let text = |e: &ast::Expr| src[offset(&src, e.start)..offset(&src, e.end)].to_string();
    assert_eq!(text(rhs), "((b + 1)) * (c)");
    let ExprKind::Binary(_, l, r) = &rhs.kind else {
        panic!("binary")
    };
    assert_eq!(text(l), "((b + 1))");
    assert_eq!(text(r), "(c)");
    ok("a := ((b + 1)) * (c);\nx := NOT ( (x) AND ( y ) );\na := - ( b );\na := +b;");
}

#[test]
fn unary_member_index_bit_and_call_shapes() {
    ok("a := -5; a := - b; a := -b ** 2; x := NOT NOT x; a := arr[ b MOD 4 , 1 ] + arr[2];");
    ok("a := b.3; arr[1] := c; a := LIMIT(0, b, 5); a := LIMIT ( IN := b , MN := 0 , MX := 3 );");
    ok("a := b * c + b / c - b MOD 3; x := a < b AND b <= c OR a <> c XOR x;");
}

#[test]
fn calls_with_argument_lists() {
    let src = "PROGRAM t VAR tm : TON; x : BOOL; et : TIME; END_VAR tm ( IN := x , PT := T#5ms , ET => et ) ; END_PROGRAM";
    assert_eq!(stmt_text(src, 0), "tm ( IN := x , PT := T#5ms , ET => et ) ;");
    check(src).unwrap();
}

/// An in-out argument is a `NAME := variable` argument and a function
/// output a `NAME => variable` one, in a call statement or a call
/// expression; comments and odd spacing between the tokens.
#[test]
fn in_out_arguments_and_function_outputs_have_exact_ranges() {
    let src = "PROGRAM t VAR a, b : ARRAY[0..3] OF INT; r, k, l, m : INT; w : bump; END_VAR\n\
w ( buf := a , n := b [ 2 ] ) ;\n\
w(buf := (* c *) a, n := r); // end\n\
k := split ( 7 , lo => l , hi => b[ k ] ) ;\n\
split(1, lo => m);\n\
END_PROGRAM\n\
FUNCTION_BLOCK bump VAR_IN_OUT buf : ARRAY[0..3] OF INT; n : INT; END_VAR n := buf[0]; END_FUNCTION_BLOCK\n\
FUNCTION split : INT VAR_INPUT v : INT; END_VAR VAR_OUTPUT lo, hi : INT; END_VAR split := v; END_FUNCTION\n";
    assert_eq!(stmt_text(src, 0), "w ( buf := a , n := b [ 2 ] ) ;");
    assert_eq!(stmt_text(src, 1), "w(buf := (* c *) a, n := r);");
    assert_eq!(stmt_text(src, 2), "k := split ( 7 , lo => l , hi => b[ k ] ) ;");
    assert_eq!(stmt_text(src, 3), "split(1, lo => m);");
    check(src).unwrap();
}

#[test]
fn the_last_statement_with_no_trailing_newline() {
    let src = "PROGRAM t VAR a : INT; END_VAR a := 1; a := 2;END_PROGRAM";
    assert_eq!(stmt_text(src, 1), "a := 2;");
    check(src).unwrap();
    let src = "PROGRAM t VAR a : INT; END_VAR a := 1;\nEND_PROGRAM";
    check(src).unwrap();
}

#[test]
fn multi_line_blocks_end_after_their_closing_keyword() {
    let body = "IF x THEN\n  a := 1;\nELSIF y THEN\n  a := 2;\nELSE\n  a := 3;\nEND_IF  ;  b := 1;\n\
CASE a OF\n  0, 1: b := 1;\n  2..5:\n    b := 2;\nELSE\n  b := 3;\nEND_CASE;\n\
FOR a := 0 TO 3 BY 1 DO\n  b := b + a; (* c *)\n  IF a = 2 THEN EXIT; END_IF;\nEND_FOR;\n\
WHILE b < 3 DO\n  b := b + 1;\n  CONTINUE;\nEND_WHILE;\n\
REPEAT\n  b := b + 1;\nUNTIL b > 5\nEND_REPEAT;\nRETURN;\n;";
    let src = prog(body);
    assert!(stmt_text(&src, 0).starts_with("IF x THEN") && stmt_text(&src, 0).ends_with("END_IF  ;"));
    assert!(stmt_text(&src, 2).ends_with("END_CASE;"));
    assert!(stmt_text(&src, 3).ends_with("END_FOR;"));
    assert!(stmt_text(&src, 4).ends_with("END_WHILE;"));
    assert!(stmt_text(&src, 5).ends_with("END_REPEAT;"));
    assert_eq!(stmt_text(&src, 6), "RETURN;");
    assert_eq!(stmt_text(&src, 7), ";");
    ok(body);
}

#[test]
fn a_block_as_the_last_statement_with_no_newline() {
    let src = "PROGRAM t VAR x : BOOL; a : INT; END_VAR IF x THEN a := 1; END_IF;END_PROGRAM";
    assert_eq!(stmt_text(src, 0), "IF x THEN a := 1; END_IF;");
    check(src).unwrap();
}

#[test]
fn library_statement_ranges_are_not_empty() {
    for pou in &ast::library().pous {
        // Library POUs use `system` names, which `ast::parse` refuses
        // to re-read; only that each range is non-empty is checked here.
        fn walk(b: &[ast::Stmt]) {
            for s in b {
                assert!(s.pos < s.end, "{:?}", s.pos);
                match &s.kind {
                    StmtKind::If(arms, els) => {
                        for (_, b) in arms {
                            walk(b);
                        }
                        walk(els);
                    }
                    StmtKind::While(_, b) | StmtKind::Repeat(b, _) | StmtKind::For { body: b, .. } => walk(b),
                    StmtKind::Case(_, arms, els) => {
                        for (_, b) in arms {
                            walk(b);
                        }
                        walk(els);
                    }
                    _ => {}
                }
            }
        }
        walk(&pou.body);
    }
}
