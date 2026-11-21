//! docs/FEATURES.md held to the canvas's tabs: every pane and bottom tab
//! is under a feature with a journey, and the manifest names no tab the
//! canvas no longer has (docs/PARITY.md). The routes and the agent
//! profiles are helmsman-plc `tests/features.rs`.

use std::collections::BTreeSet;
use std::path::Path;

use helmsman_compat::features::{self, Manifest, Surface};

use crate::editor::{Bottom, Pane};

const FILE: &str = "docs/FEATURES.md";

#[test]
fn every_tab_is_under_a_feature() {
    let text =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(FILE)).unwrap();
    let m = Manifest::parse(&text).unwrap_or_else(|e| panic!("{FILE} does not read:\n{}", e.join("\n")));
    let have: BTreeSet<Surface> = Pane::ALL
        .iter()
        .map(|p| p.key())
        .chain(Bottom::ALL.iter().map(|b| b.key()))
        .map(|k| Surface::Tab(k.into()))
        .collect();
    assert_eq!(
        have.len(),
        Pane::ALL.len() + Bottom::ALL.len(),
        "two tabs share a key"
    );
    let (missing, stale) = m.coverage(&have, |s| matches!(s, Surface::Tab(_)));
    assert!(
        missing.is_empty() && stale.is_empty(),
        "{}",
        features::explain(FILE, &missing, &stale)
    );
}
