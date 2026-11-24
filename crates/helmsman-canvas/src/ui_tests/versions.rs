//! The clock in the Describe tab (Nick, 2026-11-23, `instruct::Versions`):
//! it turns the answer area into the index of the project's versions, to
//! search and step to, and back. The tab's output can be selected.

use super::Harness;
use crate::app::App;

const LAMP: &str = crate::instruct_tests::LAMP;

fn project() -> Harness {
    let mut h = Harness::new(App::sim());
    h.app.chrome.dock(crate::editor::Pane::Claude, true);
    {
        let v = h.app.sim.as_mut().unwrap();
        v.chat.receive(crate::claude::Incoming::Proxy(true));
        v.proj.create("mine", None);
        v.proj.files.insert("main.st".into(), LAMP.into());
        v.proj.save_as("first cut of the lamp", None);
        v.proj.files.insert(
            "main.st".into(),
            LAMP.replace("lamp := start;", "lamp := NOT start;"),
        );
        v.proj.save_as("inverted for the night shift", None);
        v.open_project("mine");
    }
    h.press("tab claude");
    h.wait(0.3);
    h
}

#[test]
fn the_clock_lists_the_versions_searches_them_and_a_click_runs_one() {
    let mut h = project();
    assert!(h.has("agent versions"), "the clock is in the buttons' row");
    assert!(!h.has("agent versions find"));
    let (left, right) = (h.rect("left"), h.rect("right"));
    h.press("agent versions");
    h.wait(0.2);
    assert!(h.has("agent versions find"), "the answer area is the index now");
    let revs: Vec<u32> = h
        .app
        .sim
        .as_ref()
        .unwrap()
        .proj
        .revisions()
        .iter()
        .map(|r| r.n)
        .collect();
    let key = |n: u32| format!("revision {n}");
    for n in &revs {
        assert!(h.has(&key(*n)), "version {n} is listed");
    }
    let named = |h: &Harness, what: &str| {
        h.app
            .sim
            .as_ref()
            .unwrap()
            .proj
            .revisions()
            .iter()
            .find(|r| r.what == what)
            .map(|r| r.n)
            .expect("that version")
    };
    let first = named(&h, "first cut of the lamp");
    let last = named(&h, "inverted for the night shift");

    // Every word must be in the version's description, in any letter case.
    h.app.sim.as_mut().unwrap().instruct.versions_find = "NIGHT inverted".into();
    h.wait(0.2);
    assert!(h.has(&key(last)) && !h.has(&key(first)));
    h.app.sim.as_mut().unwrap().instruct.versions_find = "no such words here".into();
    h.wait(0.2);
    assert!(!h.has(&key(last)) && h.drew("No version says that."));
    h.app.sim.as_mut().unwrap().instruct.versions_find = "x".repeat(400);
    h.wait(0.2);
    assert!(
        (h.rect("left").right() - left.right()).abs() <= 8.0,
        "the panels keep their width"
    );
    assert!((right.left() - h.rect("right").left()).abs() <= 8.0);

    h.app.sim.as_mut().unwrap().instruct.versions_find = "first cut".into();
    h.wait(0.2);
    h.press(&key(first));
    h.wait(0.3);
    let v = h.app.sim.as_ref().unwrap();
    assert_eq!(v.proj.files["main.st"], LAMP, "that version is on show");
    assert_eq!(v.history_at, Some(first));

    // The clock again: back to the answer.
    h.press("agent versions");
    h.wait(0.2);
    assert!(!h.has("agent versions find"));
    assert!(h.has("agent result"), "what that version was");
}

#[test]
fn words_match_a_version_by_what_it_says_and_why() {
    use crate::history_ui::RevisionRow;
    let row = |n, what: &str, about: Option<&str>| RevisionRow {
        n,
        at_ms: 0,
        by: String::new(),
        what: what.into(),
        page: about.is_some(),
        about: about.map(str::to_string),
    };
    let v = crate::instruct::Versions {
        rows: vec![
            row(1, "saved", None),
            row(2, "Described: latch it", Some("the lamp now latches until stop")),
            row(3, "Renamed lamp to beacon", None),
        ],
        at: None,
    };
    let ns = |find: &str| v.matching(find).iter().map(|r| r.n).collect::<Vec<_>>();
    assert_eq!(ns(""), [1, 2, 3]);
    assert_eq!(ns("   "), [1, 2, 3]);
    assert_eq!(ns("LATCH"), [2]);
    assert_eq!(ns("until latch"), [2], "the reason counts too");
    assert_eq!(ns("lamp"), [2, 3]);
    assert_eq!(ns("lamp nowhere"), Vec::<u32>::new());
}

/// Nick, 2026-11-23: "I can't even type". The answer area, with its
/// example prompts, sits where the instruction box used to be and is the
/// biggest thing in the tab, so it is where a hand goes; it took no
/// click. A click anywhere in it puts the keyboard in the box under it,
/// and what is typed lands there. An example clicked still fills the box.
#[test]
fn a_click_on_the_answer_area_puts_the_keyboard_in_the_box() {
    use eframe::egui::Event;
    let mut h = Harness::new(App::sim());
    h.app.chrome.dock(crate::editor::Pane::Claude, true);
    h.press("tab claude");
    h.wait(0.3);
    {
        let v = h.app.sim.as_mut().unwrap();
        v.chat.receive(crate::claude::Incoming::Proxy(true));
        v.instruct.outcome = None;
        v.instruct.draft.clear();
    }
    h.wait(0.3);
    assert!(
        h.has("phrase examples"),
        "as the page opens: the examples are in the area"
    );
    let area = h.rect("agent answer area");
    let field = h.rect("agent instruction");
    assert!(
        area.height() > 4.0 * field.height(),
        "the area is the big one: {area:?} over {field:?}"
    );
    // On the area's ground, under the examples.
    h.click(eframe::egui::pos2(area.center().x, area.bottom() - 30.0));
    h.frame(vec![]);
    h.frame(vec![Event::Text("turn the lamp on".into())]);
    h.frame(vec![]);
    assert_eq!(h.app.sim.as_ref().unwrap().instruct.draft, "turn the lamp on");

    // An example, clicked: it is in the box, and typing goes on from it.
    h.app.sim.as_mut().unwrap().instruct.draft.clear();
    h.wait(0.2);
    h.press("phrase example 0");
    h.frame(vec![]);
    let example = h.app.sim.as_ref().unwrap().instruct.draft.clone();
    assert!(!example.is_empty(), "the example is in the box");
    h.frame(vec![Event::Text("!".into())]);
    h.frame(vec![]);
    let typed = h.app.sim.as_ref().unwrap().instruct.draft.clone();
    assert!(
        typed.contains('!') && typed.len() == example.len() + 1,
        "typing lands in the box: {typed:?}"
    );

    // The versions' list keeps its own clicks: its search box is typed in.
    h.press("agent versions");
    h.wait(0.2);
    h.click(h.rect("agent versions find").center());
    h.frame(vec![Event::Text("zz".into())]);
    h.frame(vec![]);
    assert_eq!(h.app.sim.as_ref().unwrap().instruct.versions_find, "zz");
}
