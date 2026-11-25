//! The box page's trend picker: devices first, a device's signals on a
//! click, and the ticks left of the lanes for acting on several at once.

use super::{App, Harness};
use crate::box_tests::{Bench, watch};
use crate::remote::SignalRow;

const MAP: &str = "device rio modbus-tcp 10.0.0.57:502 unit=1\n\
in a bool rio di:0\nin b bool rio di:1\nin c bool rio di:2\n\
in s1 bool sim x\nout lamp bool box do:0\n";

fn row(name: &str) -> SignalRow {
    SignalRow {
        name: name.into(),
        dir: "in".into(),
        unit: None,
        value: None,
        quality: "good".into(),
        point_error: None,
        forced: false,
        sim: false,
    }
}

fn page() -> Harness {
    let bench = Bench::new();
    let (mut state, _) = watch(&bench);
    state.io_map = Some(MAP.into());
    state.signals = ["a", "b", "c", "s1", "lamp", "stray"].map(row).to_vec();
    let mut h = Harness::new(App::boxed());
    h.app.boxv.as_mut().unwrap().state = state;
    h.frame(vec![]);
    h.frame(vec![]);
    h
}

fn pins(h: &Harness) -> Vec<String> {
    h.app.boxv.as_ref().unwrap().trend.pins.clone()
}

fn pin(h: &mut Harness, group: &str, names: &[&str]) {
    h.press(&format!("trend group {group}"));
    h.wait(0.2);
    for n in names {
        h.press(&format!("pin {n}"));
        h.wait(0.2);
    }
}

#[test]
fn the_picker_starts_on_devices_and_a_click_shows_a_devices_signals() {
    let mut h = page();
    for g in ["rio", "simulated", "box", "other"] {
        assert!(h.has(&format!("trend group {g}")), "group {g}");
    }
    for n in ["a", "b", "c", "s1", "lamp", "stray"] {
        assert!(!h.has(&format!("pin {n}")), "{n} is hidden at first");
    }
    h.press("trend group rio");
    h.wait(0.2);
    for n in ["a", "b", "c"] {
        assert!(h.has(&format!("pin {n}")), "{n} shown");
    }
    assert!(!h.has("pin s1"), "another device's signal stays folded");
    h.press("trend group rio");
    h.wait(0.2);
    assert!(!h.has("pin a"), "folds again");
}

#[test]
fn adding_a_signal_from_a_group_trends_it() {
    let mut h = page();
    pin(&mut h, "simulated", &["s1"]);
    assert_eq!(pins(&h), ["s1"]);
}

#[test]
fn ticked_signals_are_removed_together_and_the_rest_stay() {
    let mut h = page();
    pin(&mut h, "rio", &["a", "b", "c"]);
    assert_eq!(pins(&h), ["a", "b", "c"]);
    assert!(!h.has("trend remove"), "no actions with nothing ticked");
    h.press("trend check a");
    h.wait(0.2);
    h.press("trend check c");
    h.wait(0.2);
    h.press("trend remove");
    h.wait(0.2);
    assert_eq!(pins(&h), ["b"]);
    assert!(!h.has("trend remove"), "the ticks go with them");
}

#[test]
fn select_all_ticks_every_lane_and_untick_clears() {
    let mut h = page();
    pin(&mut h, "rio", &["a", "b"]);
    h.press("trend check all");
    h.wait(0.2);
    assert!(h.has("trend remove"));
    h.press("trend check all");
    h.wait(0.2);
    assert!(!h.has("trend remove"));
    assert_eq!(pins(&h), ["a", "b"], "nothing was removed");
}

#[test]
fn ticked_signals_take_one_colour() {
    let mut h = page();
    pin(&mut h, "rio", &["a", "b", "c"]);
    h.press("trend check a");
    h.wait(0.2);
    h.press("trend check c");
    h.wait(0.2);
    h.press("trend color all");
    h.wait(0.2);
    h.press("trend color 3");
    h.wait(0.2);
    let c = &h.app.chrome;
    assert_eq!((c.color("a"), c.color("c")), (Some(3), Some(3)));
    assert_eq!(c.color("b"), None, "the unticked one keeps its own");
}

#[test]
fn a_ticked_signal_moves_up_and_the_top_one_stays() {
    let mut h = page();
    pin(&mut h, "rio", &["a", "b", "c"]);
    h.press("trend check c");
    h.wait(0.2);
    h.press("trend up");
    h.wait(0.2);
    assert_eq!(pins(&h), ["a", "c", "b"]);
    h.press("trend down");
    h.wait(0.2);
    assert_eq!(pins(&h), ["a", "b", "c"]);
    h.press("trend down");
    h.wait(0.2);
    assert_eq!(pins(&h), ["a", "b", "c"], "the last lane cannot go down");
}

#[test]
fn the_picker_leaves_the_panels_their_width() {
    let mut h = page();
    h.app.chrome.dock(crate::editor::Pane::Io, true);
    h.wait(0.3);
    let (left, right) = (h.rect("left"), h.rect("right"));
    pin(&mut h, "rio", &["a", "b", "c"]);
    h.press("trend check a");
    h.wait(0.3);
    assert!((0.0..=8.0).contains(&(h.rect("left").right() - left.right())));
    assert!((0.0..=8.0).contains(&(right.left() - h.rect("right").left())));
}

fn monitor() -> Harness {
    let mut h = page();
    h.press("monitor");
    h.frame(vec![]);
    assert_eq!(h.app.screen, crate::app::Screen::Monitor);
    h
}

#[test]
fn the_monitor_starts_on_devices_and_shows_only_the_points_picked() {
    let mut h = monitor();
    for g in ["rio", "simulated", "box", "other"] {
        assert!(h.has(&format!("monitor group {g}")), "group {g}");
    }
    assert!(
        h.has("monitor signal a") && h.has("monitor signal c"),
        "the first device starts open"
    );
    assert!(
        !h.has("monitor signal s1"),
        "another device's points start folded"
    );
    assert!(!h.has("watched signals"), "nothing watched yet");
    assert_eq!(h.app.monitor.watched, Vec::<String>::new());
    h.press("monitor signal b");
    h.wait(0.2);
    assert!(h.has("watched signals"));
    assert_eq!(h.app.monitor.watched, ["b"], "only the picked point is watched");
    h.press("monitor signal b");
    h.wait(0.2);
    assert!(!h.has("watched signals"), "taken off again");
}

#[test]
fn the_monitors_picks_are_not_the_trends() {
    let mut h = monitor();
    assert!(h.has("monitor signal a"), "rio starts open");
    h.press("monitor signal a");
    h.wait(0.2);
    assert!(pins(&h).is_empty(), "watching a point does not trend it");
}

#[test]
fn an_empty_monitor_says_what_to_do_and_stops_once_a_point_is_watched() {
    let mut h = monitor();
    assert!(h.has("monitor pick hint"), "hint shows with nothing watched");
    h.press("monitor signal a");
    h.wait(0.2);
    assert!(!h.has("monitor pick hint"), "hint goes once a point is watched");
    h.press("monitor signal a");
    h.wait(0.2);
    assert!(
        h.has("monitor pick hint"),
        "hint returns when the last is taken off"
    );
}

#[test]
fn the_monitor_opens_its_first_device_with_nothing_watched() {
    let mut h = monitor();
    h.wait(0.2);
    assert!(h.has("monitor signal a") && h.has("monitor signal b") && h.has("monitor signal c"));
    assert!(!h.has("monitor signal s1"), "other devices stay folded");
    assert!(!h.has("monitor signal lamp"), "other devices stay folded");
    assert!(
        h.app.monitor.watched.is_empty(),
        "opening a group watches nothing"
    );
}

#[test]
fn a_fold_the_user_made_in_the_monitor_survives_later_frames() {
    let mut h = monitor();
    h.press("monitor group rio");
    h.wait(0.3);
    assert!(!h.has("monitor signal a"), "the user folded rio");
    h.wait(0.3);
    assert!(!h.has("monitor signal a"), "still folded a few frames later");
    h.press("monitor group rio");
    h.wait(0.2);
    assert!(h.has("monitor signal a"), "unfolded again by the user");
    h.press("monitor signal a");
    h.wait(0.2);
    h.press("monitor signal a");
    h.wait(0.3);
    assert!(
        h.has("monitor signal a"),
        "watching and unwatching keeps their choice"
    );
    h.press("monitor group rio");
    h.wait(0.3);
    assert!(!h.has("monitor signal a"), "a fold survives later frames");
}

#[test]
fn nothing_is_watched_until_a_point_is_clicked() {
    let mut h = monitor();
    h.wait(0.5);
    assert_eq!(h.app.monitor.watched, Vec::<String>::new());
    assert!(!h.has("watched signals"));
    h.press("monitor group simulated");
    h.wait(0.2);
    assert!(
        h.app.monitor.watched.is_empty(),
        "unfolding a group watches nothing"
    );
    h.press("monitor signal a");
    h.wait(0.2);
    assert_eq!(h.app.monitor.watched, ["a"]);
}
