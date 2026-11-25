//! Placeholder (`sim`) inputs on the box page's I/O tab (Nick,
//! 2026-11-24): an engineer sets one from its card; a device input has no
//! such control, nor has anyone who is not an engineer.

use super::{App, Harness};
use crate::box_projects::PCall;
use crate::box_tests::{Bench, watch};
use crate::remote::SignalRow;
use helmsman_iec::Value;

fn row(name: &str, value: Value, sim: bool) -> SignalRow {
    SignalRow {
        name: name.into(),
        dir: "in".into(),
        unit: None,
        value: Some(value),
        quality: "good".into(),
        point_error: None,
        forced: false,
        sim,
    }
}

/// The I/O tab of a box page with `start` (sim, off), `speed` (sim, 12)
/// and `part` (a device's), signed in as `role`.
fn page(bench: &Bench, role: &str) -> Harness {
    let (state, _) = watch(bench);
    let mut h = Harness::new(App::boxed());
    h.app.chrome.dock(crate::editor::Pane::Io, true);
    let v = h.app.boxv.as_mut().unwrap();
    v.state = state;
    v.state.signals.push(row("start", Value::Bool(false), true));
    v.state.signals.push(row("speed", Value::Int(12), true));
    v.state.signals.push(row("part", Value::Bool(false), false));
    h.app.review.as_mut().unwrap().state.session = Some(crate::review::Session {
        user: Some("alice".into()),
        role: role.into(),
    });
    h.frame(vec![]);
    h.wait(0.3);
    h
}

fn sets(h: &mut Harness) -> Vec<(String, String)> {
    std::mem::take(&mut h.app.boxv.as_mut().unwrap().psent)
        .into_iter()
        .filter(|r| matches!(r.call, PCall::SetSim(_)))
        .map(|r| (r.url, String::from_utf8(r.body.unwrap()).unwrap()))
        .collect()
}

#[test]
fn an_engineer_turns_a_sim_input_on_from_its_card() {
    let bench = Bench::new();
    let mut h = page(&bench, "engineer");
    assert!(h.app.chrome.rect("sim start").is_some(), "start has its control");
    assert!(h.app.chrome.rect("sim speed").is_some(), "speed has its control");
    assert!(
        h.app.chrome.rect("sim part").is_none(),
        "a device's input has none"
    );
    h.press("sim start");
    h.wait(0.1);
    assert_eq!(
        sets(&mut h),
        [("/api/signals/start".to_string(), r#"{"value":true}"#.to_string())]
    );
    // It goes where the program runs: the watched box or Skipper.
    assert!(PCall::SetSim("start".into()).on_node());
}

#[test]
fn only_an_engineer_has_the_control() {
    let bench = Bench::new();
    let mut h = page(&bench, "operator");
    assert!(h.app.chrome.rect("sim start").is_none());
    assert!(sets(&mut h).is_empty());
}
