//! `fold::structure`: the arms of IF, CASE and loops, and the runs of
//! statements under an author's comment. Every fold returned applies on
//! its own, with nothing skipped.

// Short names (d, v, f, n) read best against the library's own.
#![allow(clippy::many_single_char_names)]

mod common;

use common::{build, check, prog, shape};
use helmsman_diagram::fold::structure::{SHOWN, TITLE, arms, sections};
use helmsman_diagram::fold::{Fold, FoldKind, View, view};
use helmsman_diagram::{Diagram, Module};
use proptest::prelude::*;

const VARS: &str = "a, b, y, z : BOOL; n, m : INT;";

fn diagram(body: &str) -> (Diagram, String) {
    let src = prog(VARS, body);
    (build(&src).1.program().unwrap().diagram.clone(), src)
}

/// `folds` applied alone: all of them, none skipped, a laid out diagram.
fn applied(d: &Diagram, folds: &[Fold]) -> View {
    let v = view(d, folds);
    assert!(v.skipped.is_empty(), "skipped {:?} of {folds:?}", v.skipped);
    assert_eq!(v.folds.len(), folds.len());
    check(&v.diagram);
    v
}

/// Nothing was returned (the value in the failure message).
fn none<T: std::fmt::Debug>(v: impl AsRef<[T]>) {
    let v = v.as_ref();
    assert!(v.is_empty(), "{v:?}");
}

fn titles(folds: &[Fold]) -> Vec<&str> {
    folds.iter().map(|f| f.title.as_str()).collect()
}

fn section_titles(body: &str) -> Vec<String> {
    let (d, src) = diagram(body);
    let f = sections(&d, &src);
    applied(&d, &f);
    f.iter().map(|f| f.title.clone()).collect()
}

// ---- arms -----------------------------------------------------------------

#[test]
fn if_elsif_else_arms_with_two_statements_fold_under_their_labels() {
    let (d, _) =
        diagram("IF a THEN n := 1; m := 1;\nELSIF b THEN n := 2; m := 2;\nELSE n := 3; m := 3; END_IF;");
    let f = arms(&d);
    assert_eq!(titles(&f), ["IF A", "ELSIF B", "ELSE"]);
    let keys: Vec<_> = f.iter().map(|f| f.key.as_str()).collect();
    assert_eq!(keys, ["arm:0:0", "arm:0:1", "arm:0:2"]);
    assert!(f.iter().all(|f| f.kind == FoldKind::Arm && f.stmts.len() == 2));
    let v = applied(&d, &f);
    // Each arm is one block now.
    assert_eq!(v.diagram.regions.len(), 3);
    assert!(v.diagram.regions.iter().all(|r| r.body.len() == 1));
    assert!(shape(&v.diagram).contains("fold"));
}

#[test]
fn a_single_plain_statement_arm_is_not_folded() {
    let (d, _) = diagram("IF a THEN n := 1; ELSE n := 2; m := 2; END_IF;");
    assert_eq!(titles(&arms(&d)), ["ELSE"]);
    let (d, _) = diagram("IF a THEN n := 1; END_IF;\nWHILE n < 3 DO n := n + 1; END_WHILE;");
    none(arms(&d));
}

#[test]
fn a_single_statement_that_has_arms_is_folded_and_its_inner_arms_are_not_offered() {
    let (d, _) = diagram("IF a THEN\nIF b THEN n := 1; m := 1; ELSE n := 2; m := 2; END_IF;\nEND_IF;");
    let f = arms(&d);
    // The outer arm holds one statement, which has arms: foldable. The
    // arms inside it are not offered (outermost only).
    assert_eq!(titles(&f), ["IF A"]);
    let v = applied(&d, &f);
    assert_eq!(v.diagram.regions[0].body.len(), 1);
}

#[test]
fn nested_ifs_offer_the_outer_arm_only() {
    let (d, _) =
        diagram("IF a THEN\nn := 1;\nIF b THEN n := 2; m := 2; END_IF;\nELSE\nm := 0; n := 0;\nEND_IF;");
    let f = arms(&d);
    assert_eq!(titles(&f), ["IF A", "ELSE"]);
    assert!(f.iter().all(|f| f.key.starts_with("arm:0:")));
    applied(&d, &f);
}

#[test]
fn case_arms_and_its_else_fold() {
    let (d, _) =
        diagram("CASE n OF\n1, 3..5: m := 1; y := TRUE;\n2: m := 2;\nELSE m := 0; y := FALSE;\nEND_CASE;");
    let f = arms(&d);
    let t = titles(&f);
    assert_eq!(t.len(), 2, "{t:?}");
    assert!(t[0].starts_with("CASE") && t[0].contains("1, 3..5"), "{t:?}");
    assert_eq!(t[1], "ELSE");
    applied(&d, &f);
}

#[test]
fn loop_bodies_are_arms() {
    let (d, _) = diagram(
        "FOR n := 0 TO 3 DO m := m + 1; y := NOT y; END_FOR;\n\
         WHILE m < 9 DO m := m + 1; z := y; END_WHILE;\n\
         REPEAT m := m + 2; a := b; UNTIL m > 20 END_REPEAT;",
    );
    let f = arms(&d);
    assert_eq!(f.len(), 3, "{:?}", titles(&f));
    let keys: Vec<_> = f.iter().map(|f| f.key.as_str()).collect();
    assert_eq!(keys, ["arm:0:0", "arm:1:0", "arm:2:0"]);
    applied(&d, &f);
}

#[test]
fn keys_count_top_level_statements_so_they_survive_edits_elsewhere() {
    let (d, _) = diagram("y := a;\nIF b THEN n := 1; m := 1; END_IF;");
    assert_eq!(arms(&d)[0].key, "arm:1:0");
    let (d, _) = diagram("y := a;\nz := a;\nIF b THEN n := 5; m := 9; END_IF;");
    assert_eq!(arms(&d)[0].key, "arm:2:0");
}

#[test]
fn no_arms_no_folds() {
    let (d, _) = diagram("y := a AND b;\nn := 2;");
    none(arms(&d));
}

// ---- sections ---------------------------------------------------------------

#[test]
fn block_and_line_comments_name_sections_up_to_the_next_one() {
    let (d, src) = diagram(
        "y := a;\n(* Start *)\nn := 1;\nm := 2;\n// Run\nz := a;\ny := b;\n/* Stop */\nn := 3;\nm := 4;",
    );
    let f = sections(&d, &src);
    assert_eq!(titles(&f), ["Start", "Run", "Stop"]);
    assert!(f.iter().all(|f| f.kind == FoldKind::Section));
    assert_eq!(f[0].stmts.len(), 2);
    assert_eq!(f[1].stmts.len(), 2);
    assert_eq!(f[2].stmts.len(), 2);
    // The statement before the first comment is in no section.
    assert!(f.iter().all(|f| !f.stmts.contains(&d.top[0])));
    let v = applied(&d, &f);
    assert_eq!(v.diagram.top.len(), 4);
}

#[test]
fn a_section_of_one_plain_statement_is_not_folded() {
    none(section_titles("(* One *)\nn := 1;\n(* Two *)\nm := 2;"));
    assert_eq!(
        section_titles("(* One *)\nIF a THEN n := 1; END_IF;\n(* Two *)\nm := 2;"),
        ["One"]
    );
}

#[test]
fn a_multi_line_comment_names_the_section_by_its_first_line() {
    let t = section_titles("(*\n   Pump control\n   second line\n*)\nn := 1;\nm := 2;");
    assert_eq!(t, ["Pump control"]);
    let t = section_titles("/* Alarms\n   more */\nn := 1;\nm := 2;");
    assert_eq!(t, ["Alarms"]);
}

#[test]
fn two_comments_in_a_row_the_last_names_it() {
    let t = section_titles("// first\n// second\nn := 1;\nm := 2;");
    assert_eq!(t, ["second"]);
    let t = section_titles("(* first *)\n\n(* second *)\nn := 1;\nm := 2;");
    assert_eq!(t, ["second"]);
}

#[test]
fn a_blank_line_between_the_comment_and_its_statement_changes_nothing() {
    assert_eq!(section_titles("(* Gap *)\n\n\nn := 1;\nm := 2;"), ["Gap"]);
}

#[test]
fn a_trailing_comment_starts_no_section() {
    none(section_titles("n := 1; (* note *)\nm := 2;\ny := a;"));
    none(section_titles("n := 1; // note\nm := 2;\ny := a;"));
    // A comment before code on its own line is not alone either.
    none(section_titles("(* head *) n := 1;\nm := 2;"));
    // A trailing comment does not become the name of what follows.
    assert_eq!(
        section_titles("n := 1; // note\n// Real\nm := 2;\ny := a;"),
        ["Real"]
    );
}

#[test]
fn a_comment_inside_an_arm_starts_no_section() {
    let body = "IF a THEN\n// inside\nn := 1;\nm := 2;\nEND_IF;\ny := a;\nz := b;";
    none(section_titles(body));
    // And it does not end the section above it.
    let t = section_titles("// Top\nIF a THEN\n// inside\nn := 1;\nm := 2;\nEND_IF;\ny := a;");
    assert_eq!(t, ["Top"]);
}

#[test]
fn a_comment_with_no_statement_after_it_is_nothing() {
    none(section_titles("n := 1;\nm := 2;\n(* the end *)"));
    assert_eq!(section_titles("// Only\nn := 1;\nm := 2;\n// the end"), ["Only"]);
}

#[test]
fn crlf_line_endings_work() {
    let src = "PROGRAM t\r\nVAR n, m : INT; END_VAR\r\n// One\r\nn := 1;\r\nm := 2;\r\n(* Two *)\r\nn := 3;\r\nm := 4;\r\nEND_PROGRAM\r\n";
    let d = build(src).1.program().unwrap().diagram.clone();
    let f = sections(&d, src);
    assert_eq!(titles(&f), ["One", "Two"]);
    applied(&d, &f);
}

#[test]
fn non_ascii_text_in_a_comment_is_kept_and_cut_on_characters() {
    assert_eq!(
        section_titles("(* Pumpe é — Förderung *)\nn := 1;\nm := 2;"),
        ["Pumpe é — Förderung"]
    );
    // The tab shows SHOWN characters (90 since 2026-11-27, was TITLE's 40).
    let long = "é".repeat(SHOWN + 10);
    let t = section_titles(&format!("(* {long} *)\nn := 1;\nm := 2;"));
    assert_eq!(t[0].chars().count(), SHOWN);
    assert!(t[0].ends_with('…'));
    // Non-ASCII before a comment shifts bytes, not columns' meaning.
    let t = section_titles("(* ü *) n := 1; (* ö *)\n// Real\nm := 2;\ny := a;");
    assert_eq!(t, ["Real"]);
}

#[test]
fn a_title_at_the_limit_is_kept_and_one_over_is_cut() {
    // The tab: SHOWN characters (2026-11-27; it was TITLE's 40).
    let at = "x".repeat(SHOWN);
    assert_eq!(section_titles(&format!("// {at}\nn := 1;\nm := 2;")), [at]);
    let over = "x".repeat(SHOWN + 1);
    let t = section_titles(&format!("// {over}\nn := 1;\nm := 2;"));
    assert_eq!(t[0], format!("{}…", "x".repeat(SHOWN - 1)));
}

/// The key a fold is kept "folded" or "open" by still takes the first
/// TITLE characters, as before the tab showed more: a browser's saved
/// folds of a long comment still match (module docs, "Key").
#[test]
fn a_long_titles_key_is_still_cut_at_the_old_limit() {
    for n in [TITLE - 1, TITLE, TITLE + 1, SHOWN, SHOWN + 1] {
        let text = "x".repeat(n);
        let (d, src) = diagram(&format!("// {text}\nn := 1;\nm := 2;"));
        let f = sections(&d, &src);
        assert_eq!(f.len(), 1, "{n}: one section");
        let kept = if n > TITLE {
            format!("{}…", "x".repeat(TITLE - 1))
        } else {
            text.clone()
        };
        assert!(
            f[0].key.starts_with("section:") && f[0].key.ends_with(&format!(":{kept}")),
            "{n}: the key {:?} names the first {TITLE}",
            f[0].key
        );
    }
}

#[test]
fn an_empty_comment_names_nothing() {
    none(section_titles("(**)\nn := 1;\nm := 2;"));
    none(section_titles("(*   *)\nn := 1;\nm := 2;"));
    none(section_titles("//\nn := 1;\nm := 2;"));
    // The empty one does not hide the one above it... it is in between,
    // so the real one still names the section.
    assert_eq!(section_titles("// Real\n(**)\nn := 1;\nm := 2;"), ["Real"]);
}

#[test]
fn comment_text_that_looks_like_code_or_markers_is_just_text() {
    assert_eq!(
        section_titles("(* // not nested *)\nn := 1;\nm := 2;"),
        ["// not nested"]
    );
    assert_eq!(section_titles("// (* open\nn := 1;\nm := 2;"), ["(* open"]);
    // A pragma is not a comment, and what is in it is not one either.
    none(section_titles("{ (* hidden *) }\nn := 1;\nm := 2;"));
}

#[test]
fn a_pou_without_comments_has_no_sections() {
    let (d, src) = diagram("n := 1;\nm := 2;\nIF a THEN y := b; END_IF;");
    none(sections(&d, &src));
}

#[test]
fn duplicate_titles_get_an_ordinal_in_the_key() {
    let (d, src) = diagram("// Same\nn := 1;\nm := 2;\n// Same\nn := 3;\nm := 4;\n// Same\nn := 5;\nm := 6;");
    let f = sections(&d, &src);
    let keys: Vec<_> = f.iter().map(|f| f.key.as_str()).collect();
    assert_eq!(keys, ["section:T:Same", "section:T:Same#2", "section:T:Same#3"]);
    applied(&d, &f);
}

#[test]
fn the_function_block_and_the_program_each_have_sections() {
    let src = "FUNCTION_BLOCK fb\nVAR_INPUT i : BOOL; END_VAR\nVAR_OUTPUT q : BOOL; r : BOOL; END_VAR\n\
               // Outputs\nq := i;\nr := NOT i;\n\
               END_FUNCTION_BLOCK\n\
               PROGRAM p\nVAR x, y, z : BOOL; f : fb; END_VAR\n\
               x := TRUE;\n\
               (* Drive *)\nf(i := x);\ny := f.q;\n\
               /* Report */\nz := f.r;\nx := z;\n\
               END_PROGRAM\n";
    let m: Module = build(src).1;
    let fb = m.pou("fb").unwrap();
    let p = m.program().unwrap();
    let f = sections(&fb.diagram, src);
    assert_eq!(titles(&f), ["Outputs"]);
    applied(&fb.diagram, &f);
    let f = sections(&p.diagram, src);
    assert_eq!(titles(&f), ["Drive", "Report"]);
    assert_eq!(f[0].key, "section:P:Drive");
    applied(&p.diagram, &f);
}

#[test]
fn sections_and_arms_do_not_break_each_other_when_applied_together() {
    let (d, src) = diagram("// Top\nIF a THEN n := 1; m := 1; END_IF;\ny := b;");
    let mut all = sections(&d, &src);
    all.extend(arms(&d));
    let v = view(&d, &all);
    // The section holds the arm: the arm is the one left out, by
    // overlap, never a broken diagram.
    assert_eq!(v.folds.len() + v.skipped.len(), 2);
    check(&v.diagram);
}

// ---- generated bodies ---------------------------------------------------------

/// The bodies with a comment of some kind above some statements.
fn commented(body: &[String], marks: &[u8]) -> Vec<String> {
    body.iter()
        .enumerate()
        .map(|(i, s)| match marks.get(i).copied().unwrap_or(0) % 6 {
            0 => s.clone(),
            1 => format!("(* section {i} *)\n{s}"),
            2 => format!("// section {i}\n{s}"),
            3 => format!("/* section {i}\n   more */\n\n{s}"),
            4 => format!("(* a *)\n// b {i}\n{s}"),
            _ => format!("{s} (* trailing *)"),
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(150))]

    #[test]
    fn every_generated_fold_applies_alone(
        body in proptest::collection::vec(common::programs::stmt(), 1..7),
        marks in proptest::collection::vec(any::<u8>(), 0..7),
        crlf in any::<bool>(),
    ) {
        let mut src = common::programs::program(&commented(&body, &marks), false);
        if crlf {
            src = src.replace('\n', "\r\n");
        }
        let m = build(&src).1;
        for pou in &m.pous {
            let d = &pou.diagram;
            for folds in [arms(d), sections(d, &src)] {
                let v = view(d, &folds);
                prop_assert!(v.skipped.is_empty(), "{:?}\n{src}", v.skipped);
                prop_assert_eq!(v.folds.len(), folds.len());
                check(&v.diagram);
                let keys: std::collections::HashSet<_> = folds.iter().map(|f| &f.key).collect();
                prop_assert_eq!(keys.len(), folds.len(), "keys are unique");
            }
        }
    }
}
