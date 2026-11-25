//! The clock in the Code header: the same list of versions as the Agent
//! tab's, under the header; and the arrows' direction in both places:
//! left is older, right is newer.

use eframe::egui;

use super::Harness;
use crate::app::App;

const LAMP: &str = crate::instruct_tests::LAMP;

fn inverted() -> String {
    LAMP.replace("lamp := start;", "lamp := NOT start;")
}

fn project(tab: &str) -> Harness {
    let mut h = Harness::new(App::sim());
    h.app.chrome.dock(crate::editor::Pane::Claude, true);
    {
        let v = h.app.sim.as_mut().unwrap();
        v.chat.receive(crate::claude::Incoming::Proxy(true));
        v.proj.create("mine", None);
        v.proj.files.insert("main.st".into(), LAMP.into());
        v.proj.save_as("first cut of the lamp", None);
        v.proj.files.insert("main.st".into(), inverted());
        v.proj.save_as("inverted for the night shift", None);
        v.open_project("mine");
    }
    if tab == "code" {
        h.app.chrome.show_code();
        h.frame(vec![]);
    } else {
        h.press(&format!("tab {tab}"));
    }
    h.wait(0.3);
    h
}

fn main_st(h: &Harness) -> String {
    h.app.sim.as_ref().unwrap().proj.files["main.st"].clone()
}

fn revs(h: &Harness) -> Vec<u32> {
    h.app.sim.as_ref().unwrap().proj.revisions().iter().map(|r| r.n).collect()
}

#[test]
fn the_code_header_has_a_clock_when_there_are_versions() {
    let h = project("code");
    assert!(h.has("code versions"));
    assert!(!h.has("code versions list"));
    let pane = h.rect("code header rest");
    let clock = h.rect("code versions");
    assert!(clock.left() >= pane.left() - 1.0 && clock.right() <= pane.right() + 1.0);
}

#[test]
fn the_clock_opens_the_list_and_shuts_it() {
    let mut h = project("code");
    h.press("code versions");
    h.wait(0.2);
    assert!(h.has("code versions list"));
    assert!(h.has("agent versions find"), "the search box");
    for n in revs(&h) {
        assert!(h.has(&format!("revision {n}")), "version {n} is listed");
    }
    h.press("code versions");
    h.wait(0.2);
    assert!(!h.has("code versions list") && !h.has("agent versions find"));
}

#[test]
fn a_click_on_a_version_shows_it() {
    let mut h = project("code");
    assert_eq!(main_st(&h), inverted());
    let first = h
        .app
        .sim
        .as_ref()
        .unwrap()
        .proj
        .revisions()
        .iter()
        .find(|r| r.what == "first cut of the lamp")
        .map(|r| r.n)
        .expect("that version");
    h.press("code versions");
    h.wait(0.2);
    h.press(&format!("revision {first}"));
    h.wait(0.3);
    assert_eq!(main_st(&h), LAMP, "that revision's text is on show");
    assert_eq!(h.app.sim.as_ref().unwrap().history_at, Some(first));
}

#[test]
fn the_search_box_narrows_the_list_in_the_code_tab() {
    let mut h = project("code");
    h.press("code versions");
    h.wait(0.2);
    h.app.sim.as_mut().unwrap().instruct.versions_find = "NIGHT".into();
    h.wait(0.2);
    let n = revs(&h);
    let shown = n.iter().filter(|n| h.has(&format!("revision {n}"))).count();
    assert_eq!(shown, 1, "only the night-shift version matches");
}

#[test]
fn without_versions_there_is_no_clock() {
    let mut h = Harness::code();
    h.wait(0.5);
    {
        let v = h.app.sim.as_mut().unwrap();
        v.proj.create("fresh", None);
        v.open_project("fresh");
    }
    h.wait(0.5);
    if h.app.sim.as_ref().unwrap().proj.revisions().is_empty() {
        assert!(!h.has("code versions"), "no versions, no clock");
        assert!(!h.has("code back") && !h.has("code forward"));
    }
}

#[test]
fn the_header_holds_every_control_on_one_row_inside_the_pane() {
    let (mut h, _, _) = two_changes("code");
    h.wait(0.3);
    let pane = h.rect("code header rest");
    let keys: Vec<&str> = ["code file", "code find", "code wrap", "code versions", "code back", "code forward", "code deploy"]
        .into_iter()
        .filter(|k| h.has(k))
        .collect();
    for must in ["code file", "code find", "code wrap", "code versions", "code deploy"] {
        assert!(keys.contains(&must), "{must} is in the header");
    }
    let left = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(crate::editor::LEFT_W + 4.0, 10_000.0));
    let rects: Vec<_> = keys.iter().map(|k| (*k, h.rect(k))).collect();
    let y = rects[0].1.center().y;
    for (k, r) in &rects {
        assert!(r.left() >= left.left() - 1.0 && r.right() <= left.right() + 1.0, "{k} {r:?} inside {left:?}");
        assert!((r.center().y - y).abs() < r.height(), "{k} on the one row: {rects:?}");
    }
    for (i, (ka, a)) in rects.iter().enumerate() {
        for (kb, b) in &rects[i + 1..] {
            assert!(!a.shrink(1.0).intersects(b.shrink(1.0)), "{ka} {a:?} overlaps {kb} {b:?}");
        }
    }
    assert!(pane.right() <= crate::editor::LEFT_W + 4.0, "{pane:?}");
}

/// The project after two Agent changes, so the arrows have steps to walk:
/// the file is the second change, one back is the first, two back LAMP.
fn two_changes(tab: &str) -> (Harness, String, String) {
    use eframe::egui::Event;
    let mut h = project(tab);
    {
        let v = h.app.sim.as_mut().unwrap();
        v.proj.files.insert("main.st".into(), LAMP.into());
        v.proj.save_as("start again", None);
        v.open_project("mine");
    }
    h.wait(0.3);
    let first = LAMP.replace("lamp := start;", "lamp := start; (* follows start *)");
    let second = first.replace("(* follows start *)", "(* on with start *)");
    for (ask, text) in [("comment it", &first), ("reword it", &second)] {
        h.app.sim.as_mut().unwrap().instruct.draft = ask.into();
        h.press("agent send");
        h.frame(vec![]);
        let answer = format!("Why: {ask}.\n\n```st\n{text}```\n\n{}", crate::instruct_tests::follows());
        crate::instruct_tests::reply(h.app.sim.as_mut().unwrap(), &answer);
        h.wait(0.3);
        let _ = Event::Text(String::new());
    }
    assert_eq!(main_st(&h), second, "both changes landed");
    (h, first, second)
}

/// Presses `key` and says whether the file changed.
fn moves(h: &mut Harness, key: &str) -> bool {
    let before = main_st(h);
    h.press(key);
    h.wait(0.3);
    main_st(h) != before
}

fn walk_the_arrows(tab: &str, back: &str, forward: &str) {
    let (mut h, first, second) = two_changes(tab);
    assert!(h.has(back) && h.has(forward), "the arrows are there");
    assert!(
        h.rect(back).right() <= h.rect(forward).left() + 1.0,
        "left is older: {:?} {:?}",
        h.rect(back),
        h.rect(forward)
    );
    assert!(!moves(&mut h, forward), "forward at the newest does nothing");
    assert_eq!(main_st(&h), second);
    assert!(moves(&mut h, back));
    assert_eq!(main_st(&h), first, "back shows the version before");
    assert!(moves(&mut h, back));
    assert_eq!(main_st(&h), LAMP, "back again: older still");
    assert!(moves(&mut h, forward));
    assert_eq!(main_st(&h), first, "forward is newer");
    assert!(moves(&mut h, forward));
    assert_eq!(main_st(&h), second);
    assert!(!moves(&mut h, forward));
}

#[test]
fn agent_back_is_left_of_forward_and_steps_to_the_older_version() {
    walk_the_arrows("claude", "agent back", "agent forward");
}

#[test]
fn code_back_is_left_of_forward_and_steps_to_the_older_version() {
    walk_the_arrows("code", "code back", "code forward");
}
