//! Spec mode on the screen (`crate::spec_mode`): the entry in the Agent
//! tab, the specification's window and the review of a draft. At the
//! pane's default width the entry leaves the panel its width; both windows
//! stay inside a narrow page and draw nothing under the smallest text;
//! Accept and Discard act on the draft; a refused name is said beside the
//! field and the draft stays.

use eframe::egui::{Pos2, Rect, Vec2};

use super::{App, Harness};
use crate::spec_mode::{Doc, Draft, Port, Review, Row};

fn page() -> Harness {
    let mut h = Harness::new(App::sim());
    h.wait(0.3);
    h
}

fn screen(h: &Harness) -> Rect {
    Rect::from_min_size(Pos2::ZERO, h.size)
}

fn long_draft() -> Draft {
    Draft {
        program: "PROGRAM belt\nVAR_INPUT\n  start_pb : BOOL;\nEND_VAR\nVAR_OUTPUT\n  motor : BOOL;\nEND_VAR\nmotor := start_pb;\nEND_PROGRAM\n".into(),
        ports: vec![
            Port {
                name: "conveyor_jam_photo_eye_upstream".into(),
                input: true,
                ty: "BOOL".into(),
                what: Some("on while a carton blocks the eye at the upstream end of the accumulation conveyor".into()),
            },
            Port {
                name: "motor".into(),
                input: false,
                ty: "BOOL".into(),
                what: None,
            },
        ],
        tested: vec![
            Row {
                requirement: "The belt keeps running after start is released and until stop is pressed for at least one scan.".into(),
                passed: true,
                why: String::new(),
            },
            Row {
                requirement: "The motor stays on while stop is held.".into(),
                passed: false,
                why: "stays on failed at 0.12 s".into(),
            },
        ],
        kept: Vec::new(),
        untested: vec!["The belt stops within 200 ms of the guard door opening, whatever else the program is doing at that moment.".into()],
        left_out: 2,
        why: Some("the belt motor latches on start and drops on stop, as the specification's second paragraph says it must.".into()),
        warnings: vec!["Scenario The motor stays on while stop is held. (test_motor_ignores_stop.st) failed: stays on failed at 0.12 s".into()],
        docs: vec![Doc {
            name: "Pasted".into(),
            text: "The belt.".into(),
        }],
        pdfs: Vec::new(),
        on_box: false,
        label: "From a spec: Pasted".into(),
    }
}

fn inside(h: &Harness, keys: &[&str]) {
    let screen = screen(h).expand(0.5);
    for k in keys {
        let r = h.rect(k);
        assert!(
            screen.contains_rect(r),
            "{k} {r:?} is outside the page {screen:?}"
        );
    }
}

#[test]
fn the_entry_leaves_the_pane_its_width_and_opens_the_window() {
    let mut h = page();
    let left = h.rect("left");
    assert!(h.has("spec open"), "an example open: a new project");
    assert!(
        h.rect("spec open").right() <= left.right() + 0.5,
        "inside the pane"
    );
    assert!(h.small_text().is_empty(), "{:?}", h.small_text());
    h.press("spec open");
    h.wait(0.3);
    for k in [
        "spec form",
        "spec text",
        "spec add file",
        "spec total",
        "spec build",
        "spec close",
    ] {
        assert!(h.has(k), "{k}");
    }
    let grew = h.rect("left").right() - left.right();
    assert!((0.0..=8.0).contains(&grew), "the left pane grew {grew} pt");
    // Nothing typed: Build says why it cannot go.
    assert!(
        h.has("spec error") && h.drew("Paste a specification"),
        "no reason given"
    );
    assert!(h.small_text().is_empty(), "{:?}", h.small_text());
    h.press("spec close");
    h.wait(0.2);
    assert!(!h.has("spec form"));
}

#[test]
fn files_are_listed_with_a_size_and_a_remove_control_and_the_total_is_against_the_cap() {
    let mut h = page();
    let sim = h.app.sim.as_mut().unwrap();
    sim.spec.open = true;
    sim.spec.add_file("plant.md", &vec![b'a'; 3000]).unwrap();
    sim.spec.add_file("scope.txt", b"Scope.").unwrap();
    h.wait(0.3);
    assert!(h.has("spec doc 1") && h.has("spec doc 2") && h.has("spec remove 2"));
    assert!(h.drew("plant.md") && h.drew("2.9 KiB"), "name and size");
    assert!(h.drew("of 120.0 KiB used"), "the total against the cap");
    h.press("spec remove 1");
    h.wait(0.2);
    let sim = h.app.sim.as_ref().unwrap();
    assert_eq!(sim.spec.docs.len(), 1);
    assert_eq!(sim.spec.docs[0].name, "scope.txt");
    // Over the cap the window says so and Build cannot go.
    let sim = h.app.sim.as_mut().unwrap();
    sim.spec.pasted = "a".repeat(crate::spec_mode::MAX_SPEC + 10);
    h.wait(0.3);
    assert!(h.drew("one request takes at most 120 KiB"));
    let before = h.app.sim.as_ref().unwrap().link.sent.len();
    h.press("spec build");
    h.wait(0.2);
    assert_eq!(
        h.app.sim.as_ref().unwrap().link.sent.len(),
        before,
        "nothing was sent"
    );
    assert!(h.small_text().is_empty(), "{:?}", h.small_text());
}

#[test]
fn add_file_asks_the_picker_and_says_so_natively() {
    let mut h = page();
    h.app.sim.as_mut().unwrap().spec.open = true;
    h.wait(0.3);
    h.press("spec add file");
    h.wait(0.2);
    assert!(h.drew("Files are added in the browser page."));
}

#[test]
fn the_windows_stay_inside_a_narrow_page() {
    for (w, hgt) in [(760.0, 640.0), (360.0, 640.0), (1400.0, 500.0)] {
        let mut h = page();
        h.size = Vec2::new(w, hgt);
        let sim = h.app.sim.as_mut().unwrap();
        sim.spec.open = true;
        sim.spec.pasted = "line\n".repeat(200);
        sim.spec
            .add_file(&format!("{}.md", "n".repeat(100)), b"x")
            .unwrap();
        h.wait(0.4);
        inside(
            &h,
            &[
                "spec form",
                "spec text",
                "spec add file",
                "spec build",
                "spec close",
                "spec doc 1",
                "spec remove 1",
            ],
        );
        assert!(h.rect("spec form").width() <= w, "{w}");
        assert!(h.small_text().is_empty(), "{w}: {:?}", h.small_text());
        let sim = h.app.sim.as_mut().unwrap();
        sim.spec.open = false;
        sim.spec_review = Some(Review::new(long_draft(), "belt".into()));
        h.wait(0.4);
        inside(&h, &["spec review", "spec name", "spec accept", "spec discard"]);
        // The rows sit in a scroll area: they may be below its edge, never
        // wider than the page.
        let page = screen(&h).expand(0.5);
        for k in [
            "spec why",
            "spec port 1",
            "spec port 2",
            "spec tested 1",
            "spec tested 2",
            "spec untested 1",
            "spec left out",
            "spec warning 1",
        ] {
            let r = h.rect(k);
            assert!(
                r.left() >= page.left() && r.right() <= page.right(),
                "{w}: {k} {r:?}"
            );
        }
        assert!(h.rect("spec review").width() <= w, "{w}");
        assert!(h.small_text().is_empty(), "{w}: {:?}", h.small_text());
    }
}

#[test]
fn the_review_shows_the_io_the_requirements_and_what_is_not_tested_yet() {
    let mut h = page();
    h.app.sim.as_mut().unwrap().spec_review = Some(Review::new(long_draft(), "belt".into()));
    h.wait(0.4);
    assert!(h.drew("I/O (2)") && h.drew("Requirements (3)"));
    assert!(h.drew("conveyor_jam_photo_eye_upstream") && h.drew("on while a carton blocks the eye"));
    assert!(h.drew("passed") && h.drew("not tested yet") && h.drew("untested"));
    assert!(h.drew("Why: the belt motor latches"));
    assert!(h.drew("2 line(s) of the Agent's list were left out"));
    // A click on the page behind leaves the draft where it is.
    h.click(Pos2::new(3.0, 3.0));
    h.wait(0.2);
    assert!(h.app.sim.as_ref().unwrap().spec_review.is_some());
    h.press("spec discard");
    h.wait(0.2);
    let sim = h.app.sim.as_ref().unwrap();
    assert!(sim.spec_review.is_none());
    assert_eq!(
        sim.instruct.outcome.as_ref().map(|o| o.status.as_str()),
        Some("Discarded the draft from the spec. Nothing was made.")
    );
    assert!(!sim.proj.exists("belt"));
}

#[test]
fn a_refused_name_is_said_beside_the_field_and_accept_with_a_good_one_makes_the_project() {
    let mut h = page();
    h.app.sim.as_mut().unwrap().spec_review = Some(Review::new(long_draft(), "traffic".into()));
    h.wait(0.3);
    h.press("spec accept");
    h.wait(0.3);
    assert!(
        h.has("spec name error") && h.drew("built-in example"),
        "refused beside the field"
    );
    let name = h.rect("spec name");
    let said = h.rect("spec name error");
    assert!(
        said.top() >= name.bottom() - 1.0 && said.left() < name.right(),
        "{said:?} under {name:?}"
    );
    assert!(
        h.app.sim.as_ref().unwrap().spec_review.is_some(),
        "the draft stays"
    );
    assert!(h.small_text().is_empty(), "{:?}", h.small_text());
    h.app.sim.as_mut().unwrap().spec_review.as_mut().unwrap().name = "belt".into();
    h.press("spec accept");
    h.wait(0.3);
    let sim = h.app.sim.as_ref().unwrap();
    assert!(sim.spec_review.is_none());
    assert_eq!(sim.proj.open, "belt");
    assert_eq!(sim.proj.files["spec.md"], "# Pasted\n\nThe belt.\n\n");
    assert!(!h.has("spec review"));
}

#[test]
fn the_kept_spec_file_is_listed_and_opens_as_text_in_the_code_tab() {
    let mut h = page();
    h.app.sim.as_mut().unwrap().spec_review = Some(Review::new(long_draft(), "belt".into()));
    h.wait(0.3);
    h.press("spec accept");
    h.wait(0.3);
    h.app.chrome.show_code();
    h.wait(0.3);
    let sim = h.app.sim.as_mut().unwrap();
    sim.proj.open_file("spec.md");
    h.wait(0.3);
    assert!(h.drew("spec.md"), "listed among the project's files");
    assert!(h.drew("The belt."), "its text is shown");
    assert!(h.small_text().is_empty(), "{:?}", h.small_text());
}

// ---- on a box ----

/// A box's page with an engineer signed in and the box's project list
/// read (a spec needs both).
fn boxed() -> Harness {
    let mut h = Harness::new(App::boxed());
    h.app.review.as_mut().unwrap().state.session = Some(crate::review::Session {
        user: Some("alice".into()),
        role: "engineer".into(),
    });
    let b = h.app.boxv.as_mut().unwrap();
    b.projects.available = Some(true);
    b.projects.new_runs = Some(crate::box_projects::Runs {
        sizes: helmsman_iec::ImageSizes { i: 32, q: 32, m: 256 },
        period: std::time::Duration::from_millis(10),
    });
    h.wait(0.3);
    h
}

/// The run a draft on review belongs to: the Agent is held until Accept
/// or Discard.
fn on_review(h: &mut Harness, draft: Draft, name: &str) {
    let b = h.app.boxv.as_mut().unwrap();
    b.instruct.run = Some(crate::instruct::Run {
        instruction: "From a spec: Pasted".into(),
        project: String::new(),
        scope: crate::scope::Scope::NewProject,
        earlier: Vec::new(),
        asked: String::new(),
        retries: 0,
        asked_scenarios: false,
        attempts: Vec::new(),
        mode: crate::instruct::Mode::Direct,
        spec: true,
    });
    b.spec.review = Some(Review::new(draft, name.into()));
}

fn box_draft() -> Draft {
    let mut d = long_draft();
    d.ports.push(Port {
        name: "tank_level".into(),
        input: true,
        ty: "INT".into(),
        what: Some("the tank level in percent".into()),
    });
    d.for_box(vec![crate::spec_mode::PdfRecord {
        name: "manual.pdf".into(),
        size: 100,
        sha256: "0".repeat(64),
    }])
}

fn pdf(n: usize) -> Vec<u8> {
    let mut v = b"%PDF-1.4\n".to_vec();
    v.resize(n, b' ');
    v
}

#[test]
fn the_box_entry_leaves_the_panel_its_width_and_is_off_for_an_operator_with_the_reason() {
    let mut h = boxed();
    h.press("tab claude");
    h.wait(0.3);
    assert!(h.has("spec open"), "the Agent tab offers it");
    let left = h.rect("left");
    for k in 0..2 {
        h.wait(0.2);
        let grew = h.rect("left").right() - left.right();
        assert!((0.0..=8.0).contains(&grew), "{k}: the panel grew {grew} pt");
    }
    assert!(h.rect("spec open").right() <= h.rect("left").right() + 0.5, "inside its panel");
    assert!(h.small_text().is_empty(), "{:?}", h.small_text());
    h.press("spec open");
    h.wait(0.3);
    assert!(h.has("spec form") && h.has("spec add file"));
    assert!(
        h.app.boxv.as_ref().unwrap().spec.spec.on_box,
        "a box's window: PDFs and the box's caps"
    );
    h.press("spec close");
    h.wait(0.2);
    // An operator: the button does nothing and the tooltip says why.
    h.app.review.as_mut().unwrap().state.session = Some(crate::review::Session {
        user: Some("bob".into()),
        role: "operator".into(),
    });
    h.wait(0.3);
    h.press("spec open");
    h.wait(0.3);
    assert!(!h.has("spec form"), "an operator cannot open it");
    let why = h.app.boxv.as_ref().unwrap().spec_entry_blocked().unwrap();
    assert!(why.contains("only an engineer"), "{why}");
}

#[test]
fn the_box_window_lists_pdfs_with_sizes_and_totals_and_stays_inside_a_narrow_page() {
    for (w, hgt) in [(1400.0, 860.0), (760.0, 640.0), (360.0, 640.0)] {
        let mut h = boxed();
        h.size = Vec2::new(w, hgt);
        let b = h.app.boxv.as_mut().unwrap();
        b.spec.spec.open = true;
        b.spec.spec.pasted = "line\n".repeat(200);
        b.spec
            .spec
            .add_file(&format!("{}.pdf", "m".repeat(100)), &pdf(3000))
            .unwrap();
        b.spec.spec.add_file("plant.md", b"# Plant").unwrap();
        b.spec.spec.add_file("second.pdf", &pdf(2000)).unwrap();
        b.spec.spec.pdfs[1].refused =
            Some("The box could not read second.pdf: not a PDF. Nothing was sent to the agent.".into());
        h.wait(0.4);
        inside(
            &h,
            &[
                "spec form",
                "spec text",
                "spec add file",
                "spec build",
                "spec close",
                "spec pdf 1",
                "spec pdf remove 1",
                "spec doc 1",
                "spec total",
                "spec pdfs",
            ],
        );
        assert!(h.rect("spec form").width() <= w, "{w}");
        assert!(h.small_text().is_empty(), "{w}: {:?}", h.small_text());
        assert!(h.drew("PDF 2.9 KiB") && h.drew("characters"), "{w}: size and totals");
        assert!(h.drew("PDFs 2 of 5; conversation"), "{w}");
        // A PDF the box refused blocks Build and says which.
        assert!(h.has("spec error"), "{w}");
        // Taking the PDF out takes its row away.
        if w > 1000.0 {
            h.press("spec pdf remove 1");
            h.wait(0.3);
            assert_eq!(h.app.boxv.as_ref().unwrap().spec.spec.pdfs.len(), 1, "{w}");
        }
    }
}

#[test]
fn the_box_review_lists_the_inputs_that_need_a_device_and_points_at_assign() {
    for (w, hgt) in [(1400.0, 860.0), (360.0, 640.0)] {
        let mut h = boxed();
        h.size = Vec2::new(w, hgt);
        on_review(&mut h, box_draft(), "tank");
        h.wait(0.4);
        assert!(
            h.has("spec needs device"),
            "{w}: {:?}",
            h.app.chrome.hits.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>()
        );
        assert!(
            h.drew("Needs a device before it can run on the controller: tank_level")
                && h.drew("Assign to device I/O"),
            "{w}"
        );
        if w > 1000.0 {
            assert!(h.drew("not tested yet"), "the failed scenario's requirement");
        }
        inside(&h, &["spec review", "spec accept", "spec discard"]);
        let page = screen(&h).expand(0.5);
        let r = h.rect("spec needs device");
        assert!(r.left() >= page.left() && r.right() <= page.right(), "{w}: {r:?}");
        assert!(h.small_text().is_empty(), "{w}: {:?}", h.small_text());
        h.press("spec discard");
        h.wait(0.3);
        let b = h.app.boxv.as_ref().unwrap();
        assert!(b.spec.review.is_none());
        assert_eq!(
            b.instruct.outcome.as_ref().map(|o| o.status.as_str()),
            Some("Discarded the draft from the spec. Nothing was made.")
        );
        assert!(!b.instruct.busy(), "{w}");
    }
}

#[test]
fn a_draft_with_every_input_on_a_channel_does_not_ask_for_a_device() {
    let mut h = boxed();
    on_review(&mut h, long_draft().for_box(Vec::new()), "belt");
    h.wait(0.4);
    assert!(!h.has("spec needs device"));
    assert!(!h.drew("Assign to device I/O"));
}
