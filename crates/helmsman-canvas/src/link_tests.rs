//! How the canvas keeps its connection to a box: what it does when a
//! stream ends, goes quiet, is refused or the box restarts, and what it
//! shows from the box's replies while they lag. `BoxState` is driven with
//! the messages the transport would hand it (as `slow_link_tests` does),
//! and a real box API (`Bench`) where the JSON matters.

use serde_json::json;

use crate::box_tests::{Bench, watch};
use crate::monitor::{Feed as _, Fresh, STALE_AFTER};
use crate::remote::{BoxState, RETRY_SECS};
use crate::remote::{
    Cmd, Fetch, Gets, Inbox, Link, Msg, SILENT_AFTER, Stream, UNREACHABLE_AFTER, module_url,
};

const VALUES: &str = "/api/events";

fn listens(cmds: &[Cmd]) -> bool {
    cmds.contains(&Cmd::Listen(Stream::Values, VALUES.into()))
}

fn values_event(seq: u64, at_ms: u64) -> Msg {
    Msg::Event(
        Stream::Values,
        "values".into(),
        json!({ "seq": seq, "at_ms": at_ms, "signals": [], "modules": null }).to_string(),
    )
}

fn modules(names: &[&str]) -> String {
    json!(names.iter().map(|n| json!({ "name": n })).collect::<Vec<_>>()).to_string()
}

/// A state that is connected and has had a values event, polled once.
fn healthy() -> BoxState {
    let mut s = BoxState::new();
    s.poll(0.0);
    s.apply(Msg::Opened(Stream::Values), 0.1);
    s.apply(values_event(1, 100), 0.2);
    s
}

// ---- 2: the box ends every stream at 600 s ----

#[test]
fn a_stream_the_box_ends_is_opened_again_at_once_and_unannounced() {
    let mut s = healthy();
    s.apply(Msg::Failed(Stream::Values), 600.0);
    assert!(listens(&s.poll(600.0)), "no waiting for the backoff");
    assert_eq!(s.link(), Link::Connected, "no banner for a reconnect that works");
    assert_eq!(s.next_retry(), None);
    // It did work: events again, and the next end is quiet too.
    s.apply(Msg::Opened(Stream::Values), 600.1);
    s.apply(values_event(2, 600_000), 600.2);
    s.apply(Msg::Failed(Stream::Values), 1200.0);
    assert!(listens(&s.poll(1200.0)));
    assert_eq!(s.link(), Link::Connected);
}

#[test]
fn a_reconnect_that_fails_backs_off_from_the_second_failure() {
    let mut s = healthy();
    s.apply(Msg::Failed(Stream::Values), 600.0);
    assert!(listens(&s.poll(600.0)));
    s.apply(Msg::Failed(Stream::Values), 600.5);
    assert_eq!(
        s.link(),
        Link::Reconnecting {
            failures: 1,
            at: 600.5 + RETRY_SECS
        }
    );
    assert!(!listens(&s.poll(601.0)), "now it waits");
    assert!(listens(&s.poll(600.5 + RETRY_SECS)));
}

// ---- 10: a stream that opens and fails at once ----

#[test]
fn a_stream_that_opens_then_fails_at_once_still_counts_toward_unreachable() {
    let mut s = BoxState::new();
    let mut now = 0.0;
    s.poll(now);
    for _ in 0..UNREACHABLE_AFTER {
        s.apply(Msg::Opened(Stream::Values), now);
        s.apply(Msg::Failed(Stream::Values), now + 0.01);
        now += 10.0;
        assert!(listens(&s.poll(now)));
    }
    assert!(
        matches!(s.link(), Link::Unreachable { .. }),
        "{:?} after {UNREACHABLE_AFTER} opens that failed at once",
        s.link()
    );
    // The first event is what counts as working.
    s.apply(Msg::Opened(Stream::Values), now);
    s.apply(values_event(1, 5), now);
    assert_eq!(s.link(), Link::Connected);
}

// ---- 3: a connection that dies without an error ----

#[test]
fn a_stream_that_goes_silent_is_closed_and_opened_again() {
    let mut s = healthy();
    assert!(
        s.poll(SILENT_AFTER - 1.0).is_empty(),
        "quiet for a moment is not dead"
    );
    let cmds = s.poll(SILENT_AFTER + 1.0);
    assert!(cmds.contains(&Cmd::Close(Stream::Values)), "{cmds:?}");
    assert!(listens(&cmds), "{cmds:?}");
    assert!(
        matches!(s.link(), Link::Reconnecting { failures: 1, .. }),
        "it counts: {:?}",
        s.link()
    );
    // The new attempt is the browser's to time out until it opens; once
    // open and silent, it is closed again.
    assert_eq!(s.poll(2.0 * SILENT_AFTER), []);
    s.apply(Msg::Opened(Stream::Values), 2.0 * SILENT_AFTER);
    assert_eq!(s.poll(2.0 * SILENT_AFTER + 1.0), []);
    let cmds = s.poll(3.0 * SILENT_AFTER + 1.0);
    assert!(listens(&cmds), "{cmds:?}");
    assert!(matches!(s.link(), Link::Reconnecting { failures: 2, .. }));
    // Events resume: all is well again.
    s.apply(values_event(9, 900), 3.0 * SILENT_AFTER + 2.0);
    assert_eq!(s.link(), Link::Connected);
    assert_eq!(s.poll(3.0 * SILENT_AFTER + 2.0), []);
}

#[test]
fn a_box_waiting_out_a_backoff_is_not_taken_for_silent() {
    let mut s = healthy();
    s.apply(Msg::Failed(Stream::Values), 1.0);
    s.apply(Msg::Failed(Stream::Values), 1.1);
    let n = |s: &mut BoxState, t| s.poll(t).iter().filter(|c| listens(&[(*c).clone()])).count();
    assert_eq!(n(&mut s, 1.2), 1, "the quiet reopen");
    assert_eq!(
        n(&mut s, 1.1 + RETRY_SECS),
        1,
        "one retry after the backoff, not two"
    );
}

// ---- 4: a healthy box with its stream slots taken ----

#[test]
fn a_box_that_answers_fetches_is_never_taken_for_no_box() {
    let mut s = BoxState::new();
    s.poll(0.0);
    s.apply(Msg::Got(Fetch::Modules, Ok(modules(&["a"]))), 0.1);
    assert!(s.ever_connected(), "the module list is an answer");
    // Every stream refused (8 slots taken, 503): the page stays the box's.
    let mut now = 0.2;
    for _ in 0..=UNREACHABLE_AFTER {
        s.apply(Msg::Failed(Stream::Values), now);
        now += 10.0;
        s.poll(now);
    }
    assert!(s.ever_connected());
}

#[test]
fn a_box_that_never_answered_is_still_no_box() {
    let mut s = BoxState::new();
    s.poll(0.0);
    s.apply(Msg::Got(Fetch::Modules, Err("HTTP 503".into())), 0.1);
    s.apply(Msg::Got(Fetch::Modules, Ok("<html>".into())), 0.2);
    assert!(!s.ever_connected());
}

// ---- 8: a module that goes away under the selection ----

#[test]
fn a_module_that_disappears_from_the_values_stream_is_deselected() {
    let mut s = BoxState::new();
    s.poll(0.0);
    s.apply(Msg::Got(Fetch::Modules, Ok(modules(&["a", "b"]))), 0.1);
    assert_eq!(s.selected.as_deref(), Some("a"));
    s.poll(0.2);
    let ev = json!({ "seq": 1, "at_ms": 1, "signals": [], "modules": [{ "name": "b" }] });
    s.apply(Msg::Event(Stream::Values, "values".into(), ev.to_string()), 0.3);
    assert_eq!(s.selected.as_deref(), Some("b"), "not left on a module that 404s");
    let cmds = s.poll(0.4);
    assert!(
        cmds.contains(&Cmd::Get(Fetch::Module("b".into()), module_url("b"))),
        "{cmds:?}"
    );
}

// ---- 9: the Devices tab on a failing box ----

#[test]
fn a_failed_devices_read_keeps_the_last_and_marks_it_old() {
    let mut s = BoxState::new();
    s.poll(0.0);
    let devs = json!([{ "name": "e1213", "state": "up" }]).to_string();
    s.apply(Msg::Got(Fetch::Devices, Ok(devs)), 0.1);
    s.apply(
        Msg::Got(
            Fetch::IoMap,
            Ok(json!({ "text": "in a bool box di:0\n" }).to_string()),
        ),
        0.1,
    );
    assert!(!s.devices_stale() && !s.io_map_stale());
    s.apply(Msg::Got(Fetch::Devices, Err("network".into())), 5.2);
    s.apply(Msg::Got(Fetch::IoMap, Err("network".into())), 5.2);
    assert_eq!(s.devices.len(), 1, "the last read stays");
    assert_eq!(
        s.io_map.as_deref(),
        Some("in a bool box di:0\n"),
        "the map does not flicker away"
    );
    assert!(s.devices_stale() && s.io_map_stale());
    // A good read renews both.
    s.apply(Msg::Got(Fetch::Devices, Ok("[]".into())), 10.3);
    s.apply(
        Msg::Got(Fetch::IoMap, Ok(json!({ "text": "" }).to_string())),
        10.3,
    );
    assert!(!s.devices_stale() && !s.io_map_stale());
    assert_eq!(s.devices, []);
}

#[test]
fn a_box_with_no_map_says_so() {
    let mut s = BoxState::new();
    s.apply(
        Msg::Got(Fetch::IoMap, Ok(json!({ "text": "x" }).to_string())),
        0.0,
    );
    s.apply(Msg::Got(Fetch::IoMap, Err("HTTP 404".into())), 5.0);
    assert_eq!(s.io_map, None, "a 404 is the box's answer: it runs none");
    assert!(!s.io_map_stale());
}

// ---- 11: a slow module is not stale for being slow ----

#[test]
fn a_module_that_scans_every_five_seconds_is_live_between_scans() {
    let mut s = BoxState::new();
    let list = json!([{ "name": "slow", "scan_ms": 5000.0, "status": { "state": "running" } },
                      { "name": "fast", "scan_ms": 10.0, "status": { "state": "running" } }]);
    s.apply(Msg::Got(Fetch::Modules, Ok(list.to_string())), 0.0);
    for m in ["slow", "fast"] {
        let trace = json!({ "module": m, "scan": 1, "stmts": [], "values": {} });
        s.apply(
            Msg::Event(Stream::Traces, "trace".into(), trace.to_string()),
            100.0,
        );
    }
    let fresh = |s: &BoxState, name: &str, now| {
        s.programs(now)
            .into_iter()
            .find(|p| p.name == name)
            .unwrap()
            .fresh
    };
    let between = 100.0 + STALE_AFTER + 1.5;
    assert!(matches!(fresh(&s, "fast", between), Fresh::Stale { .. }));
    assert_eq!(
        fresh(&s, "slow", between),
        Fresh::Live,
        "its next scan is not due yet"
    );
    assert!(
        matches!(fresh(&s, "slow", 100.0 + 16.0), Fresh::Stale { .. }),
        "three periods and it is late"
    );
}

// ---- 5: a deploy that leaves the statements as they were ----

#[test]
fn a_deploy_that_edits_a_literal_refetches_the_source() {
    let bench = Bench::new();
    let (mut s, _) = watch(&bench);
    let before = s.lowered.as_ref().unwrap().src.clone();
    assert!(before.contains("T#400ms"));
    let generation = s.lowered.as_ref().unwrap().generation;
    s.poll(100.0);
    s.relist();
    let cmds = s.poll(100.0);
    assert!(
        cmds.contains(&Cmd::Get(Fetch::Module("cell".into()), module_url("cell"))),
        "{cmds:?}"
    );
    // Same statements, one literal different: the table does not change.
    let edited = before.replace("T#400ms", "T#900ms");
    s.apply(
        Msg::Got(
            Fetch::Module("cell".into()),
            Ok(json!({ "source": edited }).to_string()),
        ),
        100.1,
    );
    let l = s.lowered.as_ref().expect("still drawn");
    assert!(l.src.contains("T#900ms"), "the diagram draws the new constant");
    assert!(l.generation > generation, "and the view rebuilds");
}

#[test]
fn a_deploy_that_changes_the_statements_drops_the_old_shape_at_once() {
    let bench = Bench::new();
    let (mut s, seen) = watch(&bench);
    assert!(s.lowered.is_some());
    // The next scan the box sends has one statement fewer.
    let data = seen
        .iter()
        .rev()
        .find_map(|m| match m {
            Msg::Event(Stream::Trace, _, d) => Some(d.clone()),
            _ => None,
        })
        .unwrap();
    let mut j: serde_json::Value = serde_json::from_str(&data).unwrap();
    j["stmts"].as_array_mut().unwrap().pop();
    s.poll(50.0);
    s.apply(Msg::Event(Stream::Trace, "trace".into(), j.to_string()), 50.0);
    assert!(
        s.lowered.is_none(),
        "values must not be read through a shape of another program"
    );
    let cmds = s.poll(50.1);
    assert!(
        cmds.contains(&Cmd::Get(Fetch::Module("cell".into()), module_url("cell"))),
        "{cmds:?}"
    );
    // The fetch failing leaves it undrawn rather than wrong.
    s.apply(
        Msg::Got(Fetch::Module("cell".into()), Err("HTTP 500".into())),
        50.2,
    );
    assert!(s.lowered.is_none());
}

#[test]
fn the_monitor_reads_a_changed_program_through_no_old_shape() {
    let bench = Bench::new();
    let (mut s, seen) = watch(&bench);
    s.set_monitor(true);
    let data = seen
        .iter()
        .rev()
        .find_map(|m| match m {
            Msg::Event(Stream::Trace, _, d) => Some(d.clone()),
            _ => None,
        })
        .unwrap();
    s.poll(60.0);
    s.apply(Msg::Event(Stream::Traces, "trace".into(), data.clone()), 60.0);
    let src = bench.get(&module_url("cell")).1;
    s.apply(Msg::Got(Fetch::Source("cell".into()), Ok(src)), 60.1);
    assert!(s.programs["cell"].lowered.is_some());
    let mut j: serde_json::Value = serde_json::from_str(&data).unwrap();
    j["stmts"].as_array_mut().unwrap().pop();
    s.apply(Msg::Event(Stream::Traces, "trace".into(), j.to_string()), 60.2);
    assert!(s.programs["cell"].lowered.is_none());
}

#[test]
fn a_deploy_that_edits_a_literal_refetches_what_the_monitor_watches() {
    let bench = Bench::new();
    let (mut s, seen) = watch(&bench);
    s.set_monitor(true);
    let data = seen
        .iter()
        .rev()
        .find_map(|m| match m {
            Msg::Event(Stream::Trace, _, d) => Some(d.clone()),
            _ => None,
        })
        .unwrap();
    s.poll(60.0);
    s.apply(Msg::Event(Stream::Traces, "trace".into(), data), 60.0);
    let src = bench.get(&module_url("cell")).1;
    let source = serde_json::from_str::<serde_json::Value>(&src).unwrap()["source"]
        .as_str()
        .unwrap()
        .to_string();
    s.apply(Msg::Got(Fetch::Source("cell".into()), Ok(src)), 60.1);
    assert!(s.programs["cell"].lowered.is_some());
    s.poll(60.2);
    s.relist();
    let cmds = s.poll(60.3);
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Cmd::Get(Fetch::Source(n), _) if n == "cell")),
        "{cmds:?}"
    );
    let edited = json!({ "source": source.replace("T#400ms", "T#900ms") }).to_string();
    s.apply(Msg::Got(Fetch::Source("cell".into()), Ok(edited)), 60.4);
    let l = s.programs["cell"].lowered.as_ref().unwrap();
    assert!(l.src.contains("T#900ms"), "the Monitor draws the new constant");
}

// ---- 1: the transport's inbox cannot build a backlog ----

fn trace(module: &str, scan: u64) -> Msg {
    Msg::Event(
        Stream::Traces,
        "trace".into(),
        json!({ "module": module, "scan": scan, "stmts": [], "values": {} }).to_string(),
    )
}

#[test]
fn an_inbox_nobody_drains_keeps_only_the_newest_picture() {
    let mut inbox = Inbox::default();
    inbox.push(Msg::Opened(Stream::Values));
    for k in 0..10_000 {
        inbox.push(values_event(k, k));
        inbox.push(trace("a", k));
        inbox.push(trace("b", k));
    }
    let got = inbox.take();
    assert_eq!(got.len(), 4, "{got:?}");
    assert_eq!(got[0], Msg::Opened(Stream::Values));
    assert_eq!(got[1], values_event(9999, 9999), "the newest values");
    assert_eq!(got[2], trace("a", 9999), "a trace for each module, the newest");
    assert_eq!(got[3], trace("b", 9999));
    assert_eq!(inbox.take(), []);
}

#[test]
fn an_inbox_keeps_what_means_something_in_sequence() {
    let mut inbox = Inbox::default();
    inbox.push(values_event(1, 1));
    inbox.push(Msg::Failed(Stream::Values));
    inbox.push(Msg::Opened(Stream::Values));
    inbox.push(values_event(2, 2));
    inbox.push(Msg::Got(Fetch::Modules, Ok("[]".into())));
    inbox.push(Msg::Got(Fetch::Modules, Ok("[]".into())));
    // A failure between two events of a stream is not folded away.
    assert_eq!(
        inbox.take(),
        [
            values_event(1, 1),
            Msg::Failed(Stream::Values),
            Msg::Opened(Stream::Values),
            values_event(2, 2),
            Msg::Got(Fetch::Modules, Ok("[]".into())),
            Msg::Got(Fetch::Modules, Ok("[]".into())),
        ]
    );
    // The agent's state and a malformed trace are not mistaken for values.
    inbox.push(Msg::Event(Stream::Values, "agent".into(), "{}".into()));
    inbox.push(values_event(3, 3));
    inbox.push(Msg::Event(Stream::Traces, "trace".into(), "not json".into()));
    inbox.push(Msg::Event(Stream::Traces, "trace".into(), "not json".into()));
    assert_eq!(inbox.take().len(), 4);
}

// ---- 12: a slow old reply ----

#[test]
fn a_slow_old_reply_does_not_overwrite_a_newer_one() {
    let mut gets = Gets::default();
    let first = gets.issue(&Fetch::Modules);
    let second = gets.issue(&Fetch::Modules);
    let other = gets.issue(&Fetch::Devices);
    assert!(gets.accept(&Fetch::Modules, second), "the newer lands first");
    assert!(
        !gets.accept(&Fetch::Modules, first),
        "the older, late, is dropped"
    );
    assert!(gets.accept(&Fetch::Devices, other), "each kind on its own");
    // In order, both land.
    let a = gets.issue(&Fetch::Modules);
    let b = gets.issue(&Fetch::Modules);
    assert!(gets.accept(&Fetch::Modules, a) && gets.accept(&Fetch::Modules, b));
}

// ---- 7: the agent's dot follows the box ----

#[test]
fn the_values_stream_agent_event_moves_the_agent_status() {
    use crate::box_agent::Status;
    let ctx = eframe::egui::Context::default();
    let mut v = crate::box_view::BoxView::new();
    assert_eq!(v.agent.status, Status::Unread);
    let agent = |state: &str| {
        Msg::Event(
            Stream::Values,
            "agent".into(),
            json!({ "state": state, "message": "relay down" }).to_string(),
        )
    };
    v.step(&ctx, vec![agent("online")], 1.0);
    assert_eq!(v.agent.status, Status::Online);
    v.step(&ctx, vec![agent("offline")], 2.0);
    assert_eq!(v.agent.status, Status::Offline("relay down".into()));
    // The browser registers a listener for each name the stream carries.
    assert_eq!(Stream::Values.events(), ["values", "agent"]);
}
