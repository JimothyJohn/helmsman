//! docs/FEATURES.md held to the box: every route the box lists at
//! `GET /api` and every agent profile its `--help` asks the relay for is
//! under a feature with a journey, and the manifest names no route or
//! profile the box no longer has (docs/PARITY.md). The tabs are
//! helmsman-canvas `features_tests.rs`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use helmsman_compat::boxp::{self, BoxProc};
use helmsman_compat::client;
use helmsman_compat::features::{self, Manifest, Surface};
use helmsman_compat::relay;

const FILE: &str = "docs/FEATURES.md";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn manifest() -> Manifest {
    let text = std::fs::read_to_string(root().join(FILE)).unwrap();
    Manifest::parse(&text).unwrap_or_else(|e| panic!("{FILE} does not read:\n{}", e.join("\n")))
}

/// A work directory no other copy of this suite shares.
fn work(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("features-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn every_route_the_box_lists_is_under_a_feature() {
    let bin = Path::new(env!("CARGO_BIN_EXE_helmsman-plc"));
    let b = BoxProc::start(bin, &root(), &work("routes"), &[]).unwrap();
    let (status, index) = client::get(&b.url, "/api", None).unwrap();
    assert_eq!(status, 200, "GET /api: {index}\n{}", b.log_tail());
    let have: BTreeSet<Surface> = index["endpoints"]
        .as_array()
        .unwrap_or_else(|| panic!("no endpoints in {index}"))
        .iter()
        .map(|e| {
            let s = format!(
                "{} {}",
                e["method"].as_str().unwrap(),
                e["path"].as_str().unwrap()
            );
            Surface::parse(&s)
                .unwrap_or_else(|err| panic!("the box lists a route the manifest cannot write: {err}"))
        })
        .collect();
    assert!(
        have.len() > 50,
        "the box lists only {} routes: {index}",
        have.len()
    );
    let (missing, stale) = manifest().coverage(&have, |s| matches!(s, Surface::Route { .. }));
    assert!(
        missing.is_empty() && stale.is_empty(),
        "{}",
        features::explain(FILE, &missing, &stale)
    );
}

#[test]
fn every_agent_profile_the_box_asks_for_is_under_a_feature() {
    let bin = Path::new(env!("CARGO_BIN_EXE_helmsman-plc"));
    let help = boxp::help_text(bin).unwrap();
    let have: BTreeSet<Surface> = relay::default_agents(&help)
        .iter()
        .map(|(flag, _)| Surface::profile_of_flag(flag).unwrap_or_else(|| panic!("{flag} names no role")))
        .collect();
    assert!(
        have.contains(&Surface::Profile("chat".into())),
        "no --relay-agent in --help:\n{help}"
    );
    let (missing, stale) = manifest().coverage(&have, |s| matches!(s, Surface::Profile(_)));
    assert!(
        missing.is_empty() && stale.is_empty(),
        "{}",
        features::explain(FILE, &missing, &stale)
    );
}

#[test]
fn a_route_the_manifest_lacks_is_named_with_what_to_do() {
    // The message a contributor sees, so it has to say which file and what to add.
    let have: BTreeSet<Surface> = [Surface::parse("POST /api/new-thing").unwrap()].into();
    let (missing, stale) = manifest().coverage(&have, |s| matches!(s, Surface::Route { .. }));
    assert_eq!(missing, vec![Surface::parse("POST /api/new-thing").unwrap()]);
    assert!(
        stale.len() > 50,
        "every real route reads as stale against a box with one"
    );
    let m = features::explain(FILE, &missing, &stale[..1]);
    assert!(
        m.contains("docs/FEATURES.md has no feature for these") && m.contains("`POST /api/new-thing`"),
        "{m}"
    );
    assert!(m.contains("no longer has them"), "{m}");
}

/// The parity run (helmsman-compat `parity`, docs/PARITY.md) on this
/// checkout's own box: every journey written so far passes, so a dev
/// change that breaks a feature's main path fails here before it can
/// show up as a gap between dev and live.
#[test]
fn every_written_journey_passes_on_this_box() {
    use helmsman_compat::parity::{self, Outcome};
    let bin = Path::new(env!("CARGO_BIN_EXE_helmsman-plc"));
    let outcomes = parity::check(bin, &root().join(FILE), &work("journeys"), &[]).unwrap();
    let failed: Vec<String> = outcomes
        .iter()
        .filter_map(|(id, o)| match o {
            Outcome::Fail(d) => Some(format!("{id}: {d}")),
            _ => None,
        })
        .collect();
    assert!(
        failed.is_empty(),
        "journeys that failed:\n{}",
        failed.join("\n\n")
    );
    let passed = outcomes
        .iter()
        .filter(|(_, o)| matches!(o, Outcome::Pass(_)))
        .count();
    assert!(passed >= 6, "only {passed} journeys passed: {outcomes:?}");
    // No relay stands behind this box: the agent journeys are not run,
    // which is neither a pass nor a failure.
    for id in AGENT {
        let o = outcomes.iter().find(|(j, _)| j == id).map(|(_, o)| o);
        assert!(
            matches!(o, Some(Outcome::NotRun(why)) if why.contains("needs a relay")),
            "{id} without a relay: {o:?}"
        );
    }
}

const AGENT: [&str; 5] = ["explain", "requirements", "chat", "design", "help"];

/// `agent-relay-local`, the way scripts/compat.sh looks for the relay: the
/// `COMPAT_RELAY_BIN` it is given, else a build in the `agent-relay`
/// checkout beside the main helmsman checkout.
fn relay_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("COMPAT_RELAY_BIN") {
        return Some(PathBuf::from(p));
    }
    let root = root().canonicalize().ok()?;
    root.ancestors().find_map(|dir| {
        ["debug", "release"].iter().find_map(|profile| {
            let p = dir.join(format!("agent-relay/target/{profile}/agent-relay-local"));
            p.is_file().then_some(p)
        })
    })
}

/// The agent journeys (explain, requirements, chat, design, help) on this
/// checkout's box with a candidate relay in front of the stub model. It
/// does not run, and says so, when there is no relay program to find:
/// build one (`cargo build -p agent-relay-local` in agent-relay at dev)
/// and give it as `COMPAT_RELAY_BIN`; `HELMSMAN_REQUIRE_RELAY=1` turns
/// "not found" into a failure.
#[test]
fn every_agent_journey_passes_on_this_box_behind_the_stub() {
    use helmsman_compat::parity::{self, AgentSide, Outcome};
    let Some(relay_bin) = relay_bin() else {
        assert!(
            std::env::var_os("HELMSMAN_REQUIRE_RELAY").is_none(),
            "no agent-relay-local to run the agent journeys against (COMPAT_RELAY_BIN)"
        );
        eprintln!(
            "NOT RUN: the agent journeys need agent-relay-local (set COMPAT_RELAY_BIN, or build agent-relay beside this checkout)"
        );
        return;
    };
    let bin = Path::new(env!("CARGO_BIN_EXE_helmsman-plc"));
    let outcomes = parity::check_with(
        bin,
        &root().join(FILE),
        &work("agent"),
        &[],
        &AgentSide::Stub { relay_bin },
    )
    .unwrap();
    let report: Vec<String> = AGENT
        .iter()
        .map(|id| {
            let o = outcomes.iter().find(|(j, _)| j == id).map(|(_, o)| o);
            match o {
                Some(Outcome::Pass(_)) => format!("{id}: ok"),
                other => format!("{id}: {other:?}"),
            }
        })
        .collect();
    assert!(
        report.iter().all(|r| r.ends_with(": ok")),
        "agent journeys on the stub:\n{}",
        report.join("\n\n")
    );
}
