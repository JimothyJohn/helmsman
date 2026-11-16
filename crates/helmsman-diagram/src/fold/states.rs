//! A CASE on a state variable, drawn as one block of states.
//!
//! The rule ([`states`]): a CASE statement whose selector is **one plain
//! variable** (a read of a variable or of an output of a function block
//! instance, `step` or `m.state`) and which has at least two arms. It is
//! one [`FoldKind::State`] fold over the statement, with a row per arm
//! ([`Fold::rows`]) labelled with the arm's labels (`1, 3..5`, `ELSE`).
//! The canvas lights the row of the arm that ran, and a tap on a row opens
//! the fold on that arm. The variable keeps its block, wired into the
//! fold, so it is still where its value is watched.
//!
//! Left as it is:
//!
//! - **A selector that is an expression** (`step + 1`, `a[i]`, a call):
//!   not a variable, so not a state.
//! - **A selector written earlier in the same scan** (`step := 2; CASE
//!   step OF ...`): the CASE reads that assignment's value, not the
//!   variable.
//! - **A CASE of one arm**: nothing to tell apart.
//!
//! A CASE inside another CASE's arm is offered too (the fold of the outer
//! one wins while it is closed; once it is open the inner one is a block
//! of states in the arm). An empty arm is a row like any other, and an
//! ELSE with no statements is not an arm at all. Its key is
//! `state:{variable}:{n}`, `n` counting the CASEs on that variable before
//! it, so it holds while the rest of the program is edited.

use std::collections::BTreeMap;

use super::{Fold, FoldKind, FoldRow};
use crate::model::{Diagram, NodeKind, RegionKind};

/// One fold per CASE of `d` on a plain variable. See the module docs.
#[must_use]
pub fn states(d: &Diagram) -> Vec<Fold> {
    let mut seen = BTreeMap::new();
    let mut out = Vec::new();
    for (s, stmt) in d.stmts.iter().enumerate() {
        let regions: Vec<_> = stmt.arms.iter().map(|&r| d.region(r)).collect();
        if regions.len() < 2
            || !regions
                .iter()
                .all(|r| matches!(r.kind, RegionKind::Case | RegionKind::CaseElse))
        {
            continue;
        }
        // One selector for every arm, and a variable.
        let gate = regions[0].gate;
        if gate.is_none() || regions.iter().any(|r| r.gate != gate) {
            continue;
        }
        let Some(NodeKind::Read { path }) = gate.map(|(g, _)| &d.node(g).kind) else {
            continue;
        };
        let rows = regions
            .iter()
            .map(|r| FoldRow {
                label: r.label.strip_prefix("CASE ").unwrap_or(&r.label).to_string(),
                region: r.id,
            })
            .collect();
        let key = super::idioms::key(&mut seen, "state", path);
        let fold = Fold::over_stmts(d, key, FoldKind::State, path, &[s]).with_rows(rows);
        if !fold.nodes.is_empty() {
            out.push(fold);
        }
    }
    out
}
