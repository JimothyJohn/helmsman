//! `fold::ladder`: the rungs of the zoom ladder, how strongly each is
//! drawn at a zoom, and the blocks each card stands for.

mod common;

use std::collections::BTreeSet;

use common::{build, prog};
use helmsman_diagram::fold::ladder::{
    HINT, MAX_ZOOM, MIN_ZOOM, Rung, STEP, TITLE, card, groups, outline, rung_at,
};
use helmsman_diagram::{Diagram, NodeId};
use proptest::prelude::*;

const VARS: &str = "start, stop, other, motor, lamp : BOOL; count, state : INT; level, level_pct : REAL;";

const BODY: &str = "(* Motor *)
motor := (start OR motor) AND NOT stop;
IF other THEN
  count := count + 1;
  lamp := TRUE;
ELSE
  lamp := FALSE;
  count := 0;
END_IF;
(* Level *)
level_pct := LIMIT(0.0, level / 2.0 * 100.0, 100.0); (* for the screen *)
count := count + 1;";

fn diagram(body: &str) -> (Diagram, String) {
    let src = prog(VARS, body);
    (build(&src).1.program().unwrap().diagram.clone(), src)
}

/// Every zoom the camera goes to, in small steps, farthest first.
fn zooms() -> impl Iterator<Item = f32> {
    let steps = 2000_u16;
    (0..=steps).map(move |k| MIN_ZOOM * (MAX_ZOOM / MIN_ZOOM).powf(f32::from(k) / f32::from(steps)))
}

#[test]
fn the_rungs_run_from_code_to_the_program_and_wiring_is_where_a_fit_lands() {
    assert_eq!(
        Rung::ALL.map(Rung::name),
        ["Code", "Wiring", "Statements", "Sections", "Program"]
    );
    // A fit rests between 0.75 and 1.6: the blocks, with nothing formed.
    for zoom in [0.75, 1.0, 1.6] {
        assert_eq!(rung_at(zoom), Rung::Wiring, "{zoom}");
        for r in Rung::ALL {
            assert!(card(r, zoom) <= 0.0, "{r:?} at {zoom}");
        }
    }
    assert_eq!(rung_at(MAX_ZOOM), Rung::Code);
    assert_eq!(rung_at(MIN_ZOOM), Rung::Program);
    // Going out, each rung is passed once and in order.
    let mut seen = Vec::new();
    for zoom in zooms().collect::<Vec<_>>().into_iter().rev() {
        let r = rung_at(zoom);
        if seen.last() != Some(&r) {
            seen.push(r);
        }
    }
    assert_eq!(seen, Rung::ALL);
    for r in Rung::ALL {
        assert_eq!(rung_at(r.rest()), r, "{r:?} rests on itself");
        let (lo, hi) = r.span();
        assert!(lo < hi && rung_at(lo) == r && rung_at(hi) == r, "{r:?} {lo} {hi}");
    }
}

#[test]
fn the_bands_are_far_apart_and_never_two_form_at_once() {
    let bands: Vec<(f32, f32)> = Rung::ALL.iter().filter_map(|r| r.band()).collect();
    // Code forms going in, the rest going out; between one band's end and
    // the next one's start the zoom changes by a good part of STEP, and
    // from whole to whole by all of it.
    assert!(bands[0].1 > bands[0].0);
    for w in bands[1..].windows(2) {
        assert!(w[0].0 > w[0].1 && w[1].0 > w[1].1, "{w:?}");
        assert!(w[0].1 / w[1].1 >= STEP, "whole to whole: {w:?}");
        assert!(w[0].1 / w[1].0 >= 1.5, "room between: {w:?}");
    }
    assert!(bands[0].0 / bands[1].0 >= STEP, "code to statements");
    for zoom in zooms() {
        let forming = Rung::ALL
            .iter()
            .filter(|&&r| {
                let c = card(r, zoom);
                c > 0.0 && c < 1.0
            })
            .count();
        assert!(forming <= 1, "{forming} rungs forming at {zoom}");
    }
}

#[test]
fn strength_is_smooth_and_only_grows_toward_the_rung() {
    let zs: Vec<f32> = zooms().collect();
    for r in Rung::ALL {
        for w in zs.windows(2) {
            for f in [card as fn(Rung, f32) -> f32, outline] {
                let (a, b) = (f(r, w[0]), f(r, w[1]));
                assert!((0.0..=1.0).contains(&a), "{r:?} {a}");
                assert!((a - b).abs() < 0.05, "{r:?} jumps at {}: {a} {b}", w[0]);
            }
            // Going out (w[1] to w[0]) a card past the wiring only forms;
            // the code's only forms going in.
            let (far, near) = (card(r, w[0]), card(r, w[1]));
            if r == Rung::Code {
                assert!(near >= far, "{r:?} at {}", w[0]);
            } else {
                assert!(far >= near, "{r:?} at {}", w[0]);
            }
        }
        // A whole card is wholly outlined.
        for &z in &zs {
            if card(r, z) >= 1.0 {
                assert!(outline(r, z) >= 1.0 - 1e-6, "{r:?} at {z}");
            }
        }
    }
}

#[test]
fn a_group_is_outlined_faintly_before_its_card_starts() {
    // At rest on the wiring a statement is outlined, faintly, and nothing
    // farther is.
    let hint = outline(Rung::Statements, 1.0);
    assert!((hint - HINT).abs() < 1e-6, "{hint}");
    assert!(outline(Rung::Sections, 1.0) <= 0.0 && outline(Rung::Program, 1.0) <= 0.0);
    for r in [Rung::Statements, Rung::Sections, Rung::Program] {
        let (start, whole) = r.band().unwrap();
        assert!(card(r, start) <= 0.0);
        let before = outline(r, start);
        assert!(
            before > HINT && before < 1.0,
            "{r:?} firmer by its band: {before}"
        );
        assert!(outline(r, start * 1.2) < before, "{r:?} firms up on the way");
        assert!((outline(r, whole) - 1.0).abs() < 1e-6);
    }
    // On a statements view the sections it will close into are outlined.
    assert!(outline(Rung::Sections, Rung::Statements.rest()) >= HINT - 1e-6);
    assert!(outline(Rung::Program, Rung::Statements.rest()) <= 0.0);
}

#[test]
fn a_zoom_that_is_not_a_number_draws_the_wiring() {
    for z in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -1.0] {
        for r in Rung::ALL {
            let (c, o) = (card(r, z), outline(r, z));
            assert!(c.is_finite() && o.is_finite(), "{r:?} at {z}: {c} {o}");
        }
    }
    assert_eq!(rung_at(f32::NAN), Rung::Wiring);
}

fn all(d: &Diagram) -> BTreeSet<NodeId> {
    d.nodes.iter().map(|n| n.id).collect()
}

#[test]
fn a_statement_is_named_by_what_it_sets_and_keeps_its_text() {
    let (d, src) = diagram(BODY);
    let g = groups(&d, &src, Rung::Statements);
    let titles: Vec<&str> = g.iter().map(|g| g.title.as_str()).collect();
    assert_eq!(
        titles,
        ["motor", "count", "lamp", "lamp", "count", "level_pct", "count"]
    );
    let level = g.iter().find(|g| g.title == "level_pct").unwrap();
    assert_eq!(level.code, "level_pct := LIMIT(0.0, level / 2.0 * 100.0, 100.0);");
    assert_eq!(
        groups(&d, &src, Rung::Code),
        g,
        "the code is the statements, closer"
    );
    assert_eq!(groups(&d, &src, Rung::Wiring), []);
}

#[test]
fn sections_are_the_authors_then_arms_then_what_is_left() {
    let (d, src) = diagram(BODY);
    let g = groups(&d, &src, Rung::Sections);
    let titles: Vec<&str> = g.iter().map(|g| g.title.as_str()).collect();
    assert_eq!(titles, ["Motor", "Level"]);
    // No comments: the arms, and each other top-level statement.
    let (d, src) = diagram(
        "motor := start;
IF other THEN count := 1; lamp := TRUE; ELSE lamp := FALSE; count := 0; END_IF;",
    );
    let g = groups(&d, &src, Rung::Sections);
    let titles: Vec<&str> = g.iter().map(|g| g.title.as_str()).collect();
    assert_eq!(titles, ["IF OTHER", "ELSE", "motor"]);
    // One comment over the whole body is the program, not a section.
    let (d, src) = diagram("(* Everything *)\nmotor := start;\nlamp := other;");
    let g = groups(&d, &src, Rung::Sections);
    let titles: Vec<&str> = g.iter().map(|g| g.title.as_str()).collect();
    assert_eq!(titles, ["motor", "lamp"]);
}

#[test]
fn the_program_is_one_card_of_every_block() {
    let (d, src) = diagram(BODY);
    let g = groups(&d, &src, Rung::Program);
    assert_eq!(g.len(), 1);
    assert_eq!(g[0].nodes, all(&d));
    assert_eq!(g[0].title, d.pou.to_ascii_lowercase());
    // An empty body has nothing to stand for.
    let (d, src) = diagram("");
    for r in Rung::ALL {
        assert!(groups(&d, &src, r).is_empty(), "{r:?}");
    }
}

#[test]
fn a_source_that_is_not_the_diagrams_gives_only_the_program() {
    let (d, _) = diagram(BODY);
    for src in ["", "PROGRAM other\nEND_PROGRAM\n", "IF IF IF (* \u{0}"] {
        assert!(groups(&d, src, Rung::Statements).is_empty(), "{src:?}");
        assert_eq!(groups(&d, src, Rung::Program).len(), 1, "{src:?}");
        // Arms need no source; nothing else is offered and nothing panics.
        let _ = groups(&d, src, Rung::Sections);
    }
}

#[test]
fn a_long_title_is_cut() {
    let name = "a".repeat(200);
    let src = format!("PROGRAM p\nVAR {name} : BOOL; b : BOOL; END_VAR\n{name} := b;\nEND_PROGRAM\n");
    let d = build(&src).1.program().unwrap().diagram.clone();
    let g = groups(&d, &src, Rung::Statements);
    assert_eq!(g[0].title.chars().count(), TITLE);
    assert!(g[0].title.ends_with('…'));
}

proptest! {
    /// Whatever the program, no two cards of a rung share a block, every
    /// card has one, and its blocks are the diagram's.
    #[test]
    fn the_cards_of_a_rung_never_share_a_block(body in proptest::collection::vec(common::programs::stmt(), 0..8)) {
        let src = common::programs::program(&body, false);
        let (_, module) = build(&src);
        for pou in &module.pous {
            let d = &pou.diagram;
            for r in Rung::ALL {
                let mut seen = BTreeSet::new();
                for g in groups(d, &src, r) {
                    prop_assert!(!g.nodes.is_empty(), "{r:?} {}", g.key);
                    prop_assert!(g.nodes.is_subset(&all(d)));
                    for n in &g.nodes {
                        prop_assert!(seen.insert(*n), "{r:?}: {n:?} twice ({})", g.key);
                    }
                }
            }
        }
    }
}
