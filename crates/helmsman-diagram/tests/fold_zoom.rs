//! `fold::zoom` and `fold::structure::statements`: which level a zoom is
//! at, which folds a level closes given what the reader did by hand, and
//! the one fold per statement the middle level draws.

#![allow(clippy::many_single_char_names)]

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::{build, check, prog};
use helmsman_diagram::fold::structure::{LINE, arms, sections, statements};
use helmsman_diagram::fold::zoom::{
    FAR_ABOVE, FAR_BELOW, Hand, Level, NEAR_ABOVE, NEAR_BELOW, closed, level, managed,
};
use helmsman_diagram::fold::{Fold, FoldKind, view};
use helmsman_diagram::{Diagram, NodeKind};
use proptest::prelude::*;

/// Nothing was returned (the value in the failure message).
fn none<T: std::fmt::Debug>(v: impl AsRef<[T]>) {
    let v = v.as_ref();
    assert!(v.is_empty(), "{v:?}");
}

const VARS: &str = "a, b, y, z : BOOL; n, m : INT;";

fn diagram(body: &str) -> (Diagram, String) {
    let src = prog(VARS, body);
    (build(&src).1.program().unwrap().diagram.clone(), src)
}

/// What the viewer offers: sections, arms, then statements.
fn offered(d: &Diagram, src: &str) -> Vec<Fold> {
    sections(d, src)
        .into_iter()
        .filter(|f| f.stmts.len() < d.top.len())
        .chain(arms(d))
        .chain(statements(d, src))
        .collect()
}

fn no_hand(_: &Fold) -> Option<Hand> {
    None
}

const BIG: &str = "(* Start *)\nIF a THEN n := 1; m := 1; END_IF;\ny := a AND b;\n\
(* Count *)\nCASE n OF\n1: m := 1; z := TRUE;\n2: IF b THEN m := 2; n := 3; END_IF;\nELSE m := 0; z := FALSE;\nEND_CASE;\n\
n := n + 1;";

// ---- level ------------------------------------------------------------------

#[test]
fn the_thresholds_are_ordered_and_leave_a_dead_band_at_each() {
    const { assert!(FAR_BELOW < FAR_ABOVE && FAR_ABOVE < NEAR_BELOW && NEAR_BELOW < NEAR_ABOVE) };
    // The closest a fit or a jump frames at (0.75) is at the near level.
    const { assert!(NEAR_ABOVE < 0.75) };
    // The farthest a jump at the middle level frames at is inside it.
    let reach = Level::Middle.reach().unwrap();
    assert!(reach > NEAR_BELOW && reach < NEAR_ABOVE);
    assert!(Level::Far.reach().unwrap() <= FAR_BELOW);
    assert_eq!(Level::Near.reach(), None);
}

#[test]
fn a_zoom_in_the_clear_gives_its_level_whatever_it_was_at() {
    for prev in [Level::Far, Level::Middle, Level::Near] {
        assert_eq!(level(0.15, prev), Level::Far);
        assert_eq!(level(0.5, prev), Level::Middle);
        assert_eq!(level(1.0, prev), Level::Near);
        assert_eq!(level(3.0, prev), Level::Near);
    }
}

#[test]
fn inside_a_dead_band_the_level_stays() {
    for z in [FAR_BELOW, 0.33, FAR_ABOVE] {
        assert_eq!(level(z, Level::Far), Level::Far, "{z}");
        assert_eq!(level(z, Level::Middle), Level::Middle, "{z}");
    }
    for z in [NEAR_BELOW, 0.66, NEAR_ABOVE] {
        assert_eq!(level(z, Level::Middle), Level::Middle, "{z}");
        assert_eq!(level(z, Level::Near), Level::Near, "{z}");
    }
}

#[test]
fn a_zoom_that_is_not_a_number_changes_nothing() {
    for prev in [Level::Far, Level::Middle, Level::Near] {
        assert_eq!(level(f32::NAN, prev), prev);
    }
    assert_eq!(level(f32::INFINITY, Level::Far), Level::Near);
    assert_eq!(level(f32::NEG_INFINITY, Level::Near), Level::Far);
}

/// Every zoom from `from` to `to` in small steps, each at the level the
/// last one left.
fn sweep(from: f32, to: f32, start: Level) -> Vec<(f32, Level)> {
    let steps = 600_u16;
    let mut at = start;
    (0..=steps)
        .map(|i| {
            let z = from + (to - from) * f32::from(i) / f32::from(steps);
            at = level(z, at);
            (z, at)
        })
        .collect()
}

#[test]
fn sweeping_in_and_out_changes_level_once_per_threshold_each_way() {
    let up = sweep(0.15, 3.0, Level::Far);
    let levels: Vec<Level> = up.iter().map(|p| p.1).collect();
    let mut changes = levels.windows(2).filter(|w| w[0] != w[1]).count();
    assert_eq!(changes, 2, "far, middle, near");
    assert_eq!((levels[0], levels[levels.len() - 1]), (Level::Far, Level::Near));
    let down = sweep(3.0, 0.15, Level::Near);
    let levels: Vec<Level> = down.iter().map(|p| p.1).collect();
    changes = levels.windows(2).filter(|w| w[0] != w[1]).count();
    assert_eq!(changes, 2, "near, middle, far");
    assert_eq!((levels[0], levels[levels.len() - 1]), (Level::Near, Level::Far));
    // The way in changes where the way out does not: a dead band.
    let first = |s: &[(f32, Level)], to: Level| s.iter().find(|p| p.1 == to).unwrap().0;
    assert!(first(&up, Level::Near) > first(&down, Level::Middle));
    assert!(first(&up, Level::Middle) > first(&down, Level::Far));
}

#[test]
fn a_zoom_wobbling_at_a_threshold_does_not_flicker() {
    for centre in [
        f32::midpoint(FAR_BELOW, FAR_ABOVE),
        f32::midpoint(NEAR_BELOW, NEAR_ABOVE),
    ] {
        for start in [Level::Far, Level::Middle, Level::Near] {
            let mut at = level(centre, start);
            let first = at;
            for k in 0..200_u16 {
                let wobble = if k % 2 == 0 { 0.01 } else { -0.01 };
                at = level(centre + wobble, at);
                assert_eq!(at, first, "{centre} from {start:?}");
            }
        }
    }
}

proptest! {
    #[test]
    fn level_is_monotone_in_the_zoom_swept_one_way(
        a in 0.1_f32..3.5, b in 0.1_f32..3.5, start in 0_u8..3,
    ) {
        let start = [Level::Far, Level::Middle, Level::Near][usize::from(start)];
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
        let up: Vec<Level> = sweep(lo, hi, start).into_iter().map(|p| p.1).collect();
        // Going in never goes to a lower level than the one before it.
        prop_assert!(up.windows(2).all(|w| w[0] <= w[1]), "{up:?}");
        let down: Vec<Level> = sweep(hi, lo, start).into_iter().map(|p| p.1).collect();
        prop_assert!(down.windows(2).all(|w| w[0] >= w[1]), "{down:?}");
    }

    #[test]
    fn level_is_settled_in_one_call(z in 0.05_f32..4.0, start in 0_u8..3) {
        let start = [Level::Far, Level::Middle, Level::Near][usize::from(start)];
        let once = level(z, start);
        prop_assert_eq!(level(z, once), once);
    }
}

// ---- statements -------------------------------------------------------------

#[test]
fn an_empty_program_has_no_statement_folds() {
    let (d, src) = diagram("");
    none(statements(&d, &src));
    none(offered(&d, &src));
    for l in [Level::Far, Level::Middle, Level::Near] {
        assert!(closed(&[], l, &no_hand).is_empty());
    }
}

#[test]
fn one_statement_is_one_fold_titled_with_its_text() {
    let (d, src) = diagram("y := a AND b;");
    let f = statements(&d, &src);
    assert_eq!(f.len(), 1);
    assert_eq!(
        (f[0].key.as_str(), f[0].title.as_str()),
        ("stmt:0", "y := a AND b;")
    );
    assert_eq!(f[0].kind, FoldKind::Statement);
    assert!(!f[0].blocks, "a line keeps no variable as a block");
    let v = view(&d, &f);
    none(v.skipped);
    check(&v.diagram);
    // The line is the one block of the view.
    assert_eq!(v.diagram.nodes.len(), 1);
    assert!(matches!(
        v.diagram.nodes[0].kind,
        NodeKind::Fold {
            kind: FoldKind::Statement,
            ..
        }
    ));
}

#[test]
fn nested_statements_are_folds_with_paths_that_name_where_they_are() {
    let (d, src) = diagram("IF a THEN n := 1; IF b THEN m := 2; END_IF; ELSE n := 3; END_IF;\ny := a;");
    let f = statements(&d, &src);
    let keys: Vec<&str> = f.iter().map(|f| f.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "stmt:0",
            "stmt:0/0.0",
            "stmt:0/0.1",
            "stmt:0/0.1/0.0",
            "stmt:0/1.0",
            "stmt:1"
        ]
    );
    assert_eq!(f[0].title, "IF a THEN n := 1; IF b THEN m := 2; END_IF; ELS…");
    assert_eq!(f[1].title, "n := 1;");
    assert_eq!(f[3].title, "m := 2;");
    // Distinct keys, and each applies on its own.
    assert_eq!(keys.iter().collect::<BTreeSet<_>>().len(), keys.len());
    for one in &f {
        let v = view(&d, std::slice::from_ref(one));
        assert!(v.skipped.is_empty(), "{}: {:?}", one.key, v.skipped);
        check(&v.diagram);
    }
}

#[test]
fn nested_case_statements_have_paths_too() {
    let (d, src) = diagram("CASE n OF\n1: m := 1;\n2: IF b THEN m := 2; END_IF;\nELSE m := 0;\nEND_CASE;");
    let keys: Vec<String> = statements(&d, &src).into_iter().map(|f| f.key).collect();
    assert_eq!(
        keys,
        [
            "stmt:0",
            "stmt:0/0.0",
            "stmt:0/1.0",
            "stmt:0/1.0/0.0",
            "stmt:0/2.0"
        ]
    );
}

#[test]
fn a_title_is_one_line_without_comments_and_cut_at_the_limit() {
    let (d, src) = diagram(
        "IF a (* when on *) THEN\n   n := 1; // one\n   m := 1;\nEND_IF;\n\
         y := a AND b AND a AND b AND a AND b AND a AND b AND a AND b AND a AND b;",
    );
    let f = statements(&d, &src);
    assert_eq!(f[0].title, "IF a THEN n := 1; m := 1; END_IF;");
    let long = f.iter().find(|f| f.key == "stmt:1").unwrap();
    assert!(long.title.chars().count() >= LINE - 1 && long.title.chars().count() <= LINE);
    assert!(long.title.ends_with('…'));
    assert!(long.title.starts_with("y := a AND b AND"));
}

#[test]
fn a_title_with_wide_characters_is_cut_on_a_character() {
    let (d, src) = diagram(&format!("y := a; (* {} *)\nz := b;", "é".repeat(80)));
    for f in statements(&d, &src) {
        assert!(f.title.chars().count() <= LINE);
    }
}

#[test]
fn the_keys_hold_while_other_statements_are_edited() {
    let (d, src) = diagram("IF a THEN n := 1; m := 1; END_IF;\ny := a;");
    let before: BTreeMap<String, String> = statements(&d, &src)
        .into_iter()
        .map(|f| (f.key, f.title))
        .collect();
    let (d, src) = diagram("IF a THEN n := 7; m := 1; END_IF;\ny := a;");
    let after: BTreeMap<String, String> = statements(&d, &src)
        .into_iter()
        .map(|f| (f.key, f.title))
        .collect();
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>()
    );
    assert_eq!(before["stmt:1"], after["stmt:1"]);
}

#[test]
fn a_source_that_does_not_parse_or_lacks_the_pou_offers_nothing() {
    let (d, src) = diagram("y := a;");
    none(statements(&d, "PROGRAM t\nVAR\n"));
    none(statements(&d, ""));
    none(statements(&d, &src.replace("PROGRAM t", "PROGRAM other")));
    // Hostile text where a statement's span would be read.
    none(statements(&d, "\u{0}\u{ffff}(*"));
}

// ---- which folds a level closes ---------------------------------------------

fn keys_of(folds: &[Fold], pick: impl Fn(&Fold) -> bool) -> BTreeSet<String> {
    folds.iter().filter(|f| pick(f)).map(|f| f.key.clone()).collect()
}

#[test]
fn near_closes_nothing() {
    let (d, src) = diagram(BIG);
    assert!(closed(&offered(&d, &src), Level::Near, &no_hand).is_empty());
}

#[test]
fn middle_closes_every_statement_and_nothing_else() {
    let (d, src) = diagram(BIG);
    let f = offered(&d, &src);
    let shut = closed(&f, Level::Middle, &no_hand);
    assert_eq!(shut, keys_of(&f, |f| f.kind == FoldKind::Statement));
    assert!(!shut.is_empty());
    assert!(f.iter().any(|f| f.kind == FoldKind::Section) && f.iter().any(|f| f.kind == FoldKind::Arm));
}

#[test]
fn far_closes_sections_arms_and_statements() {
    let (d, src) = diagram(BIG);
    let f = offered(&d, &src);
    let shut = closed(&f, Level::Far, &no_hand);
    assert_eq!(shut, keys_of(&f, |f| managed(f.kind)));
}

#[test]
fn what_the_zoom_closes_applies_in_the_view() {
    let (d, src) = diagram(BIG);
    let f = offered(&d, &src);
    let apply = |l: Level| {
        let shut = closed(&f, l, &no_hand);
        let list: Vec<Fold> = f.iter().filter(|f| shut.contains(&f.key)).cloned().collect();
        let v = view(&d, &list);
        check(&v.diagram);
        v
    };
    let near = view(&d, &[]);
    let (far, middle) = (apply(Level::Far), apply(Level::Middle));
    // Two sections at the far level: their two blocks, and nothing outside them.
    assert_eq!(
        far.diagram.nodes.len(),
        2,
        "{:?}",
        far.diagram.nodes.iter().map(|n| &n.label).collect::<Vec<_>>()
    );
    // One line per top-level statement at the middle level.
    assert_eq!(middle.diagram.nodes.len(), d.top.len());
    none(middle.diagram.regions);
    assert!(far.diagram.nodes.len() < middle.diagram.nodes.len());
    assert!(middle.diagram.nodes.len() < near.diagram.nodes.len());
}

#[test]
fn a_program_with_no_sections_draws_its_arms_and_lines_at_far() {
    let (d, src) = diagram("IF a THEN n := 1; m := 1; END_IF;\ny := a;\nz := b;");
    let f = offered(&d, &src);
    let shut = closed(&f, Level::Far, &no_hand);
    let list: Vec<Fold> = f.iter().filter(|f| shut.contains(&f.key)).cloned().collect();
    let v = view(&d, &list);
    check(&v.diagram);
    // The arm is one block, the IF's condition stays, and the two plain
    // statements are lines.
    let folds = v
        .diagram
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Fold { .. }))
        .count();
    assert_eq!(folds, 3);
}

#[test]
fn a_fold_the_reader_set_is_left_alone_at_every_level() {
    let (d, src) = diagram(BIG);
    let f = offered(&d, &src);
    for hand_state in [Hand::Open, Hand::Closed] {
        for f0 in &f {
            let hand = |g: &Fold| (g.key == f0.key).then_some(hand_state);
            for l in [Level::Far, Level::Middle, Level::Near] {
                assert!(
                    !closed(&f, l, &hand).contains(&f0.key),
                    "{} {hand_state:?} {l:?}",
                    f0.key
                );
            }
        }
    }
}

#[test]
fn a_program_with_only_a_hand_collapsed_section_is_unchanged_by_zoom() {
    let (d, src) = diagram("(* Only *)\nn := 1;\nm := 2;");
    let f = sections(&d, &src);
    assert_eq!(f.len(), 1);
    let key = f[0].key.clone();
    let hand = |g: &Fold| (g.key == key).then_some(Hand::Closed);
    let all = offered(&d, &src);
    for l in [Level::Far, Level::Middle, Level::Near] {
        assert!(!closed(&all, l, &hand).contains(&key));
    }
}

#[test]
fn a_fold_holding_one_the_reader_opened_stays_open() {
    let (d, src) = diagram(BIG);
    let f = offered(&d, &src);
    let arm = f.iter().find(|f| f.kind == FoldKind::Arm).unwrap();
    let hand = |g: &Fold| (g.key == arm.key).then_some(Hand::Open);
    for l in [Level::Far, Level::Middle] {
        let shut = closed(&f, l, &hand);
        // Nothing that holds the arm is closed...
        for g in f
            .iter()
            .filter(|g| g.key != arm.key && arm.nodes.is_subset(&g.nodes))
        {
            assert!(!shut.contains(&g.key), "{} at {l:?}", g.key);
        }
        // ...and what is inside it, or elsewhere, still is.
        assert!(shut.iter().any(|k| k.starts_with("stmt:")), "{l:?}");
    }
    // Applied, the arm's blocks are on the view.
    let shut = closed(&f, Level::Middle, &hand);
    let list: Vec<Fold> = f.iter().filter(|f| shut.contains(&f.key)).cloned().collect();
    let v = view(&d, &list);
    check(&v.diagram);
    // No closed fold holds the arm whole; its own statements are lines.
    assert!(v.folds.iter().all(|c| !c.nodes.is_superset(&arm.nodes)));
    assert!(v.folds.iter().any(|c| c.key == "stmt:0/0.0"));
}

#[test]
fn a_hand_collapsed_arm_inside_an_if_keeps_the_if_as_it_is_set() {
    let (d, src) = diagram("IF a THEN n := 1; m := 1; END_IF;\ny := a;");
    let f = offered(&d, &src);
    let arm = f.iter().find(|f| f.kind == FoldKind::Arm).unwrap().key.clone();
    // The viewer lists the folds the reader set first.
    let hand = |g: &Fold| (g.key == arm).then_some(Hand::Closed);
    let shut = closed(&f, Level::Middle, &hand);
    let mut list: Vec<Fold> = f.iter().filter(|g| g.key == arm).cloned().collect();
    list.extend(f.iter().filter(|g| shut.contains(&g.key)).cloned());
    let v = view(&d, &list);
    check(&v.diagram);
    // The collapsed arm is one block; the IF's own line yields to it, and
    // the statement after the IF is a line.
    assert!(v.skipped.iter().any(|(i, _)| list[*i].key == "stmt:0"));
    assert!(v.folds.iter().any(|g| g.key == arm));
    assert!(v.folds.iter().any(|g| g.key == "stmt:1"));
}

#[test]
fn a_hand_set_fold_survives_a_whole_sweep_of_the_zoom() {
    let (d, src) = diagram(BIG);
    let f = offered(&d, &src);
    let hand_open: BTreeSet<String> = keys_of(&f, |f| f.kind == FoldKind::Arm);
    let hand_shut: BTreeSet<String> = keys_of(&f, |f| f.kind == FoldKind::Section);
    let hand = |g: &Fold| {
        if hand_open.contains(&g.key) {
            Some(Hand::Open)
        } else if hand_shut.contains(&g.key) {
            Some(Hand::Closed)
        } else {
            None
        }
    };
    let mut at = Level::Near;
    for (_, l) in sweep(3.0, 0.15, at)
        .into_iter()
        .chain(sweep(0.15, 3.0, Level::Far))
    {
        at = l;
        let shut = closed(&f, at, &hand);
        assert!(
            shut.is_disjoint(&hand_open) && shut.is_disjoint(&hand_shut),
            "{at:?}"
        );
    }
}

#[test]
fn closing_is_a_function_of_the_level_and_the_hand_alone() {
    let (d, src) = diagram(BIG);
    let f = offered(&d, &src);
    let mut rev = f.clone();
    rev.reverse();
    for l in [Level::Far, Level::Middle, Level::Near] {
        assert_eq!(closed(&f, l, &no_hand), closed(&rev, l, &no_hand));
        assert_eq!(closed(&f, l, &no_hand), closed(&f, l, &no_hand));
    }
}

proptest! {
    #[test]
    fn any_hand_state_leaves_a_view_that_checks(
        mask in proptest::collection::vec(0_u8..3, 0..40), l in 0_u8..3,
    ) {
        let (d, src) = diagram(BIG);
        let f = offered(&d, &src);
        let hand = |g: &Fold| {
            let i = f.iter().position(|h| h.key == g.key)?;
            match mask.get(i).copied().unwrap_or(0) {
                1 => Some(Hand::Open),
                2 => Some(Hand::Closed),
                _ => None,
            }
        };
        let l = [Level::Far, Level::Middle, Level::Near][usize::from(l)];
        let shut = closed(&f, l, &hand);
        // Hand-set folds are never in it.
        for g in &f {
            prop_assert!(hand(g).is_none() || !shut.contains(&g.key));
        }
        // The viewer puts what the reader closed first, then the zoom's.
        let list: Vec<Fold> = f
            .iter()
            .filter(|g| hand(g) == Some(Hand::Closed))
            .chain(f.iter().filter(|g| shut.contains(&g.key)))
            .cloned()
            .collect();
        let v = view(&d, &list);
        check(&v.diagram);
        // A fold the reader opened is never hidden inside a closed one.
        for g in f.iter().filter(|g| hand(g) == Some(Hand::Open)) {
            prop_assert!(v.folds.iter().all(|c| !c.nodes.is_superset(&g.nodes) || c.key == g.key || !shut.contains(&c.key)));
        }
    }
}

#[test]
fn the_far_scale_makes_every_title_fit_at_the_smallest_zoom() {
    use helmsman_diagram::fold::zoom::{FAR_FLOOR_PT, FAR_MIN_ZOOM, far_room, far_scale};
    assert_eq!(far_scale([]), (1.0, 1.0));
    // A block already big enough for its title changes nothing.
    assert_eq!(far_scale([(5, 10_000.0, 10_000.0)]), (1.0, 1.0));
    // Hostile sizes (zero, a very long title) give a finite scale.
    let (sx, sy) = far_scale([(0, 0.0, 0.0), (10_000, 56.0, 40.0), (3, 56.0, 40.0)]);
    assert!(sx.is_finite() && sx >= 1.0 && sy.is_finite() && sy >= 1.0);
    for (chars, w, h) in [(5, 56.0_f32, 40.0_f32), (8, 120.0, 100.0), (30, 300.0, 60.0)] {
        let (sx, sy) = far_scale([(chars, w, h)]);
        let (need_w, need_h) = far_room(chars);
        assert!(w * sx * FAR_MIN_ZOOM >= need_w, "{chars}");
        assert!(h * sy * FAR_MIN_ZOOM >= need_h, "{chars}");
        assert!(need_h >= FAR_FLOOR_PT);
    }
}
