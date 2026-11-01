//! The box's trend history (`api::trend`): what it keeps of a pinned
//! signal, within which bounds, and what it makes of hostile asks. Fed
//! published views directly, on a clock the test sets.

use std::time::Duration;

use helmsman_io::Value;
use helmsman_plc::api::live::View;
use helmsman_plc::api::trend::{
    LEASE, MAX_NAME, MAX_OWNERS, MAX_PINS, MAX_PINS_FILE, MAX_POINTS, MAX_TRACKED, Trends, WINDOW, Wanted,
};
use proptest::prelude::*;

fn names(n: usize) -> Vec<String> {
    (0..n).map(|k| format!("s{k}")).collect()
}

fn view(seq: u64, at: Duration, values: &[Option<Value>]) -> View {
    View {
        seq,
        at,
        signals: values.to_vec(),
        ..View::default()
    }
}

fn s(n: &str) -> String {
    n.to_string()
}

/// How many points the box keeps for `name` (a history request).
fn points(t: &mut Trends, name: &str, known: &[String]) -> usize {
    let w = t.want(&[s(name)], known);
    assert_eq!(w.kept, [s(name)]);
    t.history_len(name)
}

#[test]
fn a_signal_keeps_its_changes_within_the_window_and_the_value_it_opens_on() {
    let known = names(2);
    let mut t = Trends::default();
    t.want(&known[..1], &known);
    // A change every second for twenty minutes.
    for k in 0..1200u64 {
        let v = Some(Value::Int(i64::try_from(k).unwrap()));
        t.sample(&view(k + 1, Duration::from_secs(k), &[v, None]), &known);
    }
    let n = points(&mut t, "s0", &known);
    let window = usize::try_from(WINDOW.as_secs()).unwrap();
    assert!(
        (window..=window + 1).contains(&n),
        "{n} points over a {window} s window"
    );
    // Unchanged values add nothing.
    let before = t.history_len("s0");
    for k in 1200..1300u64 {
        t.sample(
            &view(k + 1, Duration::from_secs(k), &[Some(Value::Int(1199)), None]),
            &known,
        );
    }
    assert!(t.history_len("s0") <= before, "steps, not samples");
}

#[test]
fn a_fast_signal_keeps_at_most_max_points() {
    let known = names(1);
    let mut t = Trends::default();
    t.want(&known, &known);
    for k in 0..(MAX_POINTS as u64 * 2) {
        let v = Some(Value::Bool(k % 2 == 0));
        t.sample(&view(k + 1, Duration::from_millis(k * 10), &[v]), &known);
    }
    assert_eq!(t.history_len("s0"), MAX_POINTS);
}

#[test]
fn the_box_tracks_at_most_max_tracked_signals_and_refuses_the_next() {
    let known = names(MAX_TRACKED + 5);
    let mut t = Trends::default();
    let w = t.want(&known, &known);
    assert_eq!(w.kept.len(), MAX_TRACKED);
    assert_eq!(w.refused.len(), 5);
    assert_eq!(t.tracked().len(), MAX_TRACKED);
    // Asking again for one kept is fine; a new one is still refused.
    let w = t.want(&[s("s0"), s(&format!("s{MAX_TRACKED}"))], &known);
    assert_eq!(
        w,
        Wanted {
            kept: vec![s("s0")],
            unknown: vec![],
            refused: vec![s(&format!("s{MAX_TRACKED}"))],
        }
    );
}

#[test]
fn unknown_names_are_listed_and_never_tracked() {
    let known = names(2);
    let mut t = Trends::default();
    let hostile = [
        s("nosuch"),
        s("S0"),
        s("s0\n"),
        s("../s0"),
        s("s0,s1"),
        s(&"x".repeat(10_000)),
        s("s1"),
        s("s1"),
    ];
    let w = t.want(&hostile, &known);
    assert_eq!(w.kept, [s("s1")], "names match exactly; duplicates once");
    assert_eq!(w.unknown.len(), 6);
    assert_eq!(t.tracked(), [s("s1")]);
}

#[test]
fn a_lease_ends_a_day_after_the_last_ask_unless_someone_pins_it() {
    let known = names(2);
    let mut t = Trends::default();
    t.want(&known, &known);
    t.set_pins("user:alice", &[s("s1")], &known);
    let later = LEASE + Duration::from_secs(1);
    t.sample(&view(1, later, &[None, None]), &known);
    assert_eq!(t.tracked(), [s("s1")], "s0's lease ran out; s1 is pinned");
    assert_eq!(t.pins("user:alice"), [s("s1")]);
    t.set_pins("user:alice", &[], &known);
    assert_eq!(t.pins("user:alice"), Vec::<String>::new());
    t.sample(&view(2, later * 3, &[None, None]), &known);
    assert_eq!(t.tracked(), Vec::<String>::new(), "unpinned, its lease ends too");
}

#[test]
fn the_box_keeps_at_most_max_owners_pins() {
    let known = names(1);
    let mut t = Trends::default();
    for k in 0..=MAX_OWNERS {
        t.set_pins(&format!("user:u{k}"), &known, &known);
    }
    assert_eq!(t.pins("user:u0"), Vec::<String>::new(), "the least recent went");
    assert_eq!(t.pins(&format!("user:u{MAX_OWNERS}")), known);
}

proptest! {
    /// Whatever the views and asks, the bounds hold and nothing panics.
    #[test]
    fn the_bounds_hold_whatever_comes(
        asks in prop::collection::vec(prop::collection::vec("[a-z0-9]{1,4}", 0..20), 1..10),
        steps in prop::collection::vec((0u64..5000, prop::option::of(-5i64..5)), 0..300),
    ) {
        let known: Vec<String> = (0..100).map(|k| format!("{k}")).collect();
        let mut t = Trends::default();
        let mut at = 0u64;
        for (seq, (dt, v)) in steps.iter().enumerate() {
            if seq % 30 == 0 {
                t.want(&asks[seq / 30 % asks.len()], &known);
            }
            at += dt;
            let vals = vec![v.map(Value::Int); known.len()];
            t.sample(&view(seq as u64 + 1, Duration::from_millis(at), &vals), &known);
        }
        prop_assert!(t.tracked().len() <= MAX_TRACKED);
        for n in t.tracked() {
            prop_assert!(t.history_len(&n) <= MAX_POINTS);
        }
    }
}

/// A fresh folder for one test's pins file.
fn folder(what: &str) -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "helmsman-trend-{what}-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Card trend-pins-persist: pins live in the state directory's
/// `trend-pins` file and come back after a restart, tracked from the
/// first scan; history is memory only and starts over.
#[test]
fn pins_survive_a_restart_in_the_file_and_history_does_not() {
    let known = names(3);
    let file = folder("pins").join("trend-pins");
    {
        let mut t = Trends::in_file(file.clone());
        assert_eq!(t.file(), Some(file.as_path()));
        assert_eq!(t.pins("user:alice"), Vec::<String>::new());
        let w = t.set_pins("user:alice", &[s("s0"), s("s2")], &known);
        assert_eq!(w.kept, [s("s0"), s("s2")]);
        assert_eq!(t.save_error(), None);
        t.set_pins("role:operator", &[s("s1")], &known);
        for k in 0..100u64 {
            let v = Some(Value::Int(i64::try_from(k).unwrap()));
            t.sample(&view(k + 1, Duration::from_secs(k), &[v, v, v]), &known);
        }
        assert!(t.history_len("s0") > 50, "history while running");
    }
    assert!(file.is_file(), "the pins file exists");
    // The box restarts: the pins are back, nothing is tracked until the
    // first view says what the signals are, and history starts over.
    let mut t = Trends::in_file(file.clone());
    assert_eq!(t.pins("user:alice"), [s("s0"), s("s2")]);
    assert_eq!(t.pins("role:operator"), [s("s1")]);
    assert_eq!(t.tracked(), Vec::<String>::new(), "known only after a view");
    assert_eq!(t.history_len("s0"), 0, "history is not kept");
    t.sample(
        &view(1, Duration::from_secs(0), &[Some(Value::Int(7)), None, None]),
        &known,
    );
    assert_eq!(
        t.tracked(),
        [s("s0"), s("s1"), s("s2")],
        "pins tracked from the first scan"
    );
    assert_eq!(t.history_len("s0"), 1);
    // A pin for a signal this box no longer has stays in the file, untracked.
    let fewer = names(1);
    let mut t = Trends::in_file(file.clone());
    t.sample(&view(1, Duration::from_secs(0), &[None]), &fewer);
    assert_eq!(t.tracked(), [s("s0")]);
    assert_eq!(t.pins("user:alice"), [s("s0"), s("s2")]);
    // Unpinning reaches the file too.
    t.set_pins("user:alice", &[], &fewer);
    t.set_pins("role:operator", &[], &fewer);
    let t = Trends::in_file(file.clone());
    assert_eq!(t.pins("user:alice"), Vec::<String>::new());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{}");
}

#[test]
fn a_hostile_pins_file_starts_with_no_pins_or_only_what_fits() {
    let known = names(2);
    let dir = folder("hostile");
    let file = dir.join("trend-pins");
    let empty = |why: &str| {
        let mut t = Trends::in_file(file.clone());
        t.sample(&view(1, Duration::from_secs(0), &[None, None]), &known);
        assert_eq!(t.tracked(), Vec::<String>::new(), "{why}");
        assert_eq!(t.pins("user:alice"), Vec::<String>::new(), "{why}");
    };
    empty("no file yet");
    std::fs::write(&file, b"\xff\xfe not json").unwrap();
    empty("not json");
    std::fs::write(&file, b"[\"s0\"]").unwrap();
    empty("not an object");
    std::fs::write(
        &file,
        r#"{"user:alice": "s0", "": ["s0"], "user:bob": [1, null, ""]}"#,
    )
    .unwrap();
    empty("owners and names of the wrong shape");
    let huge = format!(
        r#"{{"user:alice": ["s0"], "pad": ["{}"]}}"#,
        "x".repeat(MAX_PINS_FILE)
    );
    std::fs::write(&file, huge).unwrap();
    empty("over the file cap");
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    empty("a folder in its place");
    std::fs::remove_dir(&file).unwrap();
    // Too many of everything: trimmed to the bounds, nothing else kept.
    let long = "n".repeat(MAX_NAME + 1);
    let mut owners = serde_json::Map::new();
    for k in 0..MAX_OWNERS + 50 {
        let mut pins: Vec<serde_json::Value> = (0..MAX_PINS + 5).map(|j| format!("p{j}").into()).collect();
        if k == 0 {
            pins.insert(0, long.clone().into());
        }
        pins.insert(1, "p0".into());
        owners.insert(format!("user:u{k:04}"), pins.into());
    }
    std::fs::write(&file, serde_json::Value::Object(owners).to_string()).unwrap();
    let t = Trends::in_file(file.clone());
    let kept: Vec<usize> = (0..MAX_OWNERS + 50)
        .map(|k| t.pins(&format!("user:u{k:04}")).len())
        .collect();
    assert_eq!(
        kept.iter().filter(|n| **n > 0).count(),
        MAX_OWNERS,
        "owners capped"
    );
    assert!(kept.iter().all(|n| *n <= MAX_PINS), "pins capped: {kept:?}");
    let first = t.pins("user:u0000");
    assert!(first.iter().all(|n| n.len() <= MAX_NAME), "{first:?}");
    assert_eq!(first.iter().filter(|n| *n == "p0").count(), 1, "deduplicated");
}

#[test]
fn pins_that_cannot_reach_the_file_stay_in_memory_and_say_why() {
    let known = names(1);
    let file = folder("nowrite").join("missing-folder").join("trend-pins");
    let mut t = Trends::in_file(file);
    let w = t.set_pins("user:alice", &[s("s0")], &known);
    assert_eq!(w.kept, [s("s0")]);
    assert_eq!(t.pins("user:alice"), [s("s0")], "held in memory");
    let why = t.save_error().expect("the failed write is reported");
    assert!(why.contains("trend-pins"), "{why}");
}
