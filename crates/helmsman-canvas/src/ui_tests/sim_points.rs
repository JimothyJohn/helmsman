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
    assert_eq!(sets(&mut h), []);
}

/// Nick 2026-11-25: "The reset state button should only be applied to
/// simulated I/O when running on an actual device." On a Skipper's page
/// Reset sets each simulated input back to off or 0 and restarts nothing;
/// a device's input is not touched.
#[test]
fn on_a_skipper_reset_sets_the_simulated_inputs_back_and_restarts_nothing() {
    let bench = Bench::new();
    let mut h = page(&bench, "engineer");
    let v = h.app.boxv.as_mut().unwrap();
    v.watches_node = true;
    v.state.signals[0].value = Some(Value::Bool(true)); // start, on
    assert_eq!(v.sim_inputs(), 2, "start and speed");
    v.reset_sims();
    h.wait(0.1);
    let mut got = sets(&mut h);
    got.sort();
    assert_eq!(
        got,
        [
            ("/api/signals/speed".to_string(), r#"{"value":0}"#.to_string()),
            ("/api/signals/start".to_string(), r#"{"value":false}"#.to_string()),
        ]
    );
    let restarts: Vec<_> = std::mem::take(&mut h.app.boxv.as_mut().unwrap().psent)
        .into_iter()
        .filter(|r| r.url.ends_with("/restart"))
        .collect();
    assert!(restarts.is_empty(), "{restarts:?}");
}
