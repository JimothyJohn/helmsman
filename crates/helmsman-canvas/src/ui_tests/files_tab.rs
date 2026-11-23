//! The Files tab's header and its two sections (card fb-files-tab): which
//! file system it shows, Examples and My Files folding with a click and
//! staying folded across a restart of the view, at the panel's default
//! width.

use super::{App, Harness};
use crate::editor::Chrome;
use crate::projects::Place;

fn files() -> Harness {
    let mut h = Harness::new(App::sim());
    h.app.chrome.show_files();
    h.wait(0.3);
    h
}

#[test]
fn the_header_names_this_browser_on_the_page() {
    let h = files();
    assert!(h.has("files place"));
    assert!(h.drew(Place::Browser.title()));
    assert_eq!(Place::Browser.title(), "Files in this browser");
}

#[test]
fn examples_and_own_projects_land_in_their_sections() {
    let mut h = files();
    assert!(h.has("files head examples") && h.has("files head mine"));
    assert!(h.has("project blink"), "an example under Examples");
    assert!(!h.has("project mine"));
    h.app.sim.as_mut().unwrap().proj.create("mine", None);
    h.wait(0.3);
    assert!(h.has("project mine"), "the visitor's own under My Files");
    // My Files comes first, then the Examples under their own heading.
    let mine = h.rect("files head mine").top();
    let own = h.rect("project mine").top();
    let ex = h.rect("files head examples").top();
    let example = h.rect("project blink").top();
    assert!(mine < own && own < ex && ex < example);
    assert!(h.drew("My Files") && h.drew("Examples"));
}

#[test]
fn a_click_folds_a_section_and_the_fold_is_remembered() {
    let mut h = files();
    h.app.sim.as_mut().unwrap().proj.create("mine", None);
    h.wait(0.3);
    h.press("files head examples");
    h.wait(0.3);
    assert!(!h.has("project blink"), "the examples are folded away");
    assert!(h.has("project mine"), "My Files stays open");
    assert!(h.has("files head examples"), "the heading stays to unfold it");
    assert!(h.app.chrome.files_fold_examples && !h.app.chrome.files_fold_mine);

    // A restart of the view: what was kept is read back.
    let kept = h.app.chrome.saved();
    let mut again = Harness::new(App::sim());
    again.app.chrome = Chrome::restored(&kept);
    again.app.chrome.show_files();
    again.app.sim.as_mut().unwrap().proj.create("mine", None);
    again.wait(0.3);
    assert!(!again.has("project blink"), "still folded");
    assert!(again.has("project mine"));

    again.press("files head examples");
    again.wait(0.3);
    assert!(again.has("project blink"), "unfolded by a click");
    again.press("files head mine");
    again.wait(0.3);
    assert!(!again.has("project mine"));
    let back = Chrome::restored(&again.app.chrome.saved());
    assert!(back.files_fold_mine && !back.files_fold_examples);
    // Nonsense kept: read as far as it makes sense. Nothing kept: both open.
    let odd = Chrome::restored("filefold=111 iofold=00");
    assert!(odd.files_fold_mine && !odd.files_fold_examples);
    let none = Chrome::restored("");
    assert!(!none.files_fold_mine && !none.files_fold_examples);
}

#[test]
fn long_names_leave_the_panel_its_default_width() {
    let mut h = files();
    let left = h.rect("left");
    for i in 0..3 {
        let name = format!("a-very-long-project-name-that-keeps-going-and-going-for-a-line-or-two-{i}");
        h.app.sim.as_mut().unwrap().proj.create(&name, None);
        h.wait(0.3);
        let grew = h.rect("left").right() - left.right();
        assert!(
            (0.0..=8.0).contains(&grew),
            "after {name} the pane grew {grew} pt"
        );
    }
    for key in [
        "files head mine",
        "files head examples",
        "files head mine",
        "files head examples",
    ] {
        h.press(key);
        h.wait(0.3);
        let grew = h.rect("left").right() - left.right();
        assert!((0.0..=8.0).contains(&grew), "after {key} the pane grew {grew} pt");
    }
}
